use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde_json::Value;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

// Removed mimalloc to simplify Docker builds on low-RAM instances

mod blackbox;
mod models;
mod utils;
mod vscode;

use blackbox::BlackboxProvider;
use models::ChatCompletionRequest;

struct AppState {
    provider: BlackboxProvider,
    proxy_api_key: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    let state = Arc::new(AppState {
        provider: BlackboxProvider::new(client),
        proxy_api_key: env::var("PROXY_API_KEY").unwrap_or_else(|_| "xyz".to_string()),
    });

    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .route("/responses", post(handle_chat_completions))
        .route("/models", get(handle_models))
        .route("/docs", get(swagger_ui))
        .route("/docs/", get(swagger_ui))
        .route("/openapi.yaml", get(openapi_yaml))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();

    tracing::info!("🚀 Blackbox Provider running at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handle_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut body): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut incoming_key = auth_header.strip_prefix("Bearer ").unwrap_or(&auth_header).trim().to_string();

    if !state.proxy_api_key.is_empty() {
        if incoming_key != state.proxy_api_key && incoming_key != "minimax-no-key-required" {
            if !incoming_key.starts_with("sk-") && !incoming_key.starts_with("cus_") {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": "Unauthorized: Invalid PROXY_API_KEY" })),
                ).into_response();
            }
        }
        
        if incoming_key == state.proxy_api_key {
            incoming_key = "".to_string();
        }
    }

    let api_key = if incoming_key.is_empty() { None } else { Some(incoming_key) };

    // Normalize tools (simple implementation)
    if let Some(tools) = &mut body.tools {
        let mut valid_tools = Vec::new();
        for t in tools.iter() {
            match t {
                models::ChatToolDefinition::Strict { .. } => {
                    valid_tools.push(t.clone());
                }
                models::ChatToolDefinition::Loose(val) => {
                    if let Some(obj) = val.as_object() {
                        let type_str = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let inner = obj.get("function").or_else(|| obj.get(type_str)).unwrap_or(val);
                        if let Some(inner_obj) = inner.as_object() {
                            if let Some(name_val) = inner_obj.get("name").or_else(|| obj.get("name")).or_else(|| obj.get("type")) {
                                if let Some(name) = name_val.as_str() {
                                    valid_tools.push(models::ChatToolDefinition::Strict {
                                        r#type: "function".to_string(),
                                        function: models::FunctionDefinition {
                                            name: name.to_string(),
                                            description: inner_obj.get("description").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                            parameters: inner_obj.get("parameters").cloned(),
                                        },
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        body.tools = if valid_tools.is_empty() { None } else { Some(valid_tools) };
    }

    match state.provider.post_chat(api_key, body).await {
        Ok(res) => {
            let mut response_builder = axum::response::Response::builder()
                .status(res.status());
            
            for (k, v) in res.headers().iter() {
                response_builder = response_builder.header(k, v);
            }
            
            let stream = res.bytes_stream();
            let body = axum::body::Body::from_stream(stream);
            response_builder.body(body).unwrap()
        }
        Err(e) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            ).into_response()
        }
    }
}

async fn handle_models() -> impl IntoResponse {
    let models = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "minimax-m2.7",
                "object": "model",
                "owned_by": "minimax",
                "free": true
            },
            {
                "id": "kimi-k2.6",
                "object": "model",
                "owned_by": "moonshot",
                "free": true
            },
            {
                "id": "custom/blackbox-base-2",
                "object": "model",
                "owned_by": "blackbox",
                "free": true
            },
            {
                "id": "gpt-4o-mini",
                "object": "model",
                "owned_by": "openai",
                "free": true
            }
        ]
    });
    Json(models)
}

async fn swagger_ui() -> Html<&'static str> {
    Html(r#"
<!DOCTYPE html>
<html>
  <head>
    <title>Blackbox Proxy API - Swagger UI</title>
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css" />
  </head>
  <body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js" crossorigin></script>
    <script>
      window.onload = () => {
        window.ui = SwaggerUIBundle({
          url: '/openapi.yaml',
          dom_id: '#swagger-ui',
        });
      };
    </script>
  </body>
</html>
"#)
}

async fn openapi_yaml() -> &'static str {
    include_str!("openapi.yaml")
}
