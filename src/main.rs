use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde_json::Value;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

mod blackbox;
mod models;
mod utils;
mod vscode;

use blackbox::BlackboxProvider;
use models::ChatCompletionRequest;

struct AppState {
    provider: BlackboxProvider,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = Arc::new(AppState {
        provider: BlackboxProvider::new(),
    });

    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .route("/responses", post(handle_chat_completions))
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
    let mut auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let proxy_secret = env::var("PROXY_API_KEY").unwrap_or_else(|_| "xyz".to_string());
    if !proxy_secret.is_empty() {
        let incoming_key = auth_header.replace("Bearer ", "").trim().to_string();
        if incoming_key != proxy_secret && incoming_key != "minimax-no-key-required" {
            // Allow the VS Code bypass logic to handle real sk- keys or proxy_secret
            // If it's a real sk- key, the user passes it directly.
            // But wait, the original proxy checks this:
            if !incoming_key.starts_with("sk-") && !incoming_key.starts_with("cus_") {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": "Unauthorized: Invalid PROXY_API_KEY" })),
                ).into_response();
            }
        }
        
        if incoming_key == proxy_secret {
            auth_header = "".to_string();
        }
    }

    let api_key = if auth_header.is_empty() {
        None
    } else {
        Some(auth_header.replace("Bearer ", "").trim().to_string())
    };

    // Normalize tools (simple implementation)
    if let Some(tools) = &mut body.tools {
        let mut valid_tools = Vec::new();
        for t in tools.iter() {
            if let Some(obj) = t.as_object() {
                if obj.get("type").and_then(|v| v.as_str()) == Some("function") 
                    && obj.get("function").and_then(|v| v.as_object()).map_or(false, |f| f.contains_key("name")) 
                {
                    valid_tools.push(t.clone());
                } else {
                    let inner = obj.get("function").or_else(|| obj.get(obj.get("type").and_then(|v| v.as_str()).unwrap_or(""))).unwrap_or(t);
                    if let Some(inner_obj) = inner.as_object() {
                        if let Some(name) = inner_obj.get("name").or_else(|| obj.get("name")).or_else(|| obj.get("type")) {
                            valid_tools.push(serde_json::json!({
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "description": inner_obj.get("description"),
                                    "parameters": inner_obj.get("parameters")
                                }
                            }));
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
