use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

// Removed mimalloc to simplify Docker builds on low-RAM instances

pub mod blackbox;
pub mod fallback;
pub mod kilo;
pub mod models;
pub mod utils;
pub mod vscode;

use blackbox::BlackboxProvider;
use kilo::KiloProvider;
use models::ChatCompletionRequest;

pub struct AppState {
    pub provider: BlackboxProvider,
    pub kilo_provider: KiloProvider,
    pub proxy_api_key: String,
}

/// Normalize the `Authorization` header into an incoming key.
/// Returns `None` when no usable credential was supplied.
pub fn extract_bearer_key(auth_header: Option<&str>) -> Option<String> {
    let raw = auth_header?.trim();
    if raw.is_empty() {
        return None;
    }
    // Strip the "Bearer" scheme case-insensitively (RFC 7235 marks the
    // scheme as case-insensitive; previously only exact "Bearer " matched,
    // so "bearer <key>" or "Bearer\t<key>" were treated as raw keys).
    let key = match raw.get(..6) {
        Some(prefix) if prefix.eq_ignore_ascii_case("bearer") => &raw[6..],
        _ => raw,
    };
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

/// Validate the incoming proxy key. `Ok(())` means the request may proceed.
///
/// The key must equal `proxy_api_key`, be the internal minimax placeholder,
/// or be a real upstream credential (`sk-...` for Blackbox API keys, or a
/// `cus_...` customer token). Previously any string merely *starting* with
/// `sk-`/`cus_` bypassed authentication entirely — an arbitrary client could
/// smuggle its own credential past the gate without knowing the proxy key.
pub fn validate_proxy_auth(incoming_key: Option<&str>, proxy_api_key: &str) -> Result<(), ()> {
    if proxy_api_key.is_empty() {
        return Ok(());
    }
    match incoming_key {
        // Exact proxy key or the known no-key placeholder are accepted.
        Some(k) if k == proxy_api_key || k == "minimax-no-key-required" => Ok(()),
        // Real upstream credentials must be well-formed, not just prefixed.
        Some(k) if is_valid_upstream_credential(k) => Ok(()),
        _ => Err(()),
    }
}

fn is_valid_upstream_credential(key: &str) -> bool {
    if let Some(rest) = key.strip_prefix("sk-") {
        // Blackbox API keys: hex/alnum ids of realistic length.
        !rest.is_empty() && rest.len() >= 20 && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    } else if let Some(rest) = key.strip_prefix("cus_") {
        // Blackbox customer tokens.
        !rest.is_empty() && rest.len() >= 10 && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    } else {
        false
    }
}

/// Turn an upstream failure into a clean JSON error response.
///
/// Fixes two problems: (1) transport errors used to surface as opaque
/// 500s, and (2) upstream bodies that were HTML/error pages (e.g. Cloud Run
/// 404 "The request Cloud Run instance no longer exists") were forwarded
/// verbatim, which clients could not parse. Non-JSON upstream bodies are now
/// wrapped into a structured OpenAI-style error object with a 502 status.
pub fn normalize_upstream_error(err: String) -> axum::response::Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": {
                "message": err,
                "type": "upstream_error",
                "code": "upstream_request_failed"
            }
        })),
    )
        .into_response()
}

/// Forward an upstream response, normalizing non-JSON error bodies.
pub async fn relay_upstream_response(res: reqwest::Response) -> axum::response::Response {
    let status = res.status();
    let content_type = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let looks_like_json = content_type.contains("json");

    if status.is_success() || looks_like_json {
        // Pass through streaming/success responses untouched.
        let mut builder = axum::response::Response::builder().status(status);
        for (k, v) in res.headers().iter() {
            builder = builder.header(k, v);
        }
        let body = axum::body::Body::from_stream(res.bytes_stream());
        return builder.body(body).unwrap();
    }

    // Error status with a non-JSON body (HTML gateway page, plain text):
    // read it and wrap into structured JSON so clients get parseable errors.
    let body_text = res.text().await.unwrap_or_default();
    // Strip markup so raw HTML never leaks into the JSON message.
    let stripped = strip_html_tags(&body_text);
    let snippet: String = stripped.trim().chars().take(300).collect();
    tracing::warn!("Upstream returned {} with non-JSON body: {}", status, snippet);
    let message = format!(
        "Upstream returned HTTP {status}: {}",
        if snippet.is_empty() {
            "(empty body)".to_string()
        } else {
            snippet
        }
    );
    normalize_upstream_error(message)
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

async fn handle_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut body): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let incoming_key = extract_bearer_key(auth_header.as_deref());

    if validate_proxy_auth(incoming_key.as_deref(), &state.proxy_api_key).is_err() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "message": "Unauthorized: Invalid PROXY_API_KEY",
                    "type": "invalid_request_error",
                    "code": "invalid_api_key"
                }
            })),
        )
            .into_response();
    }

    // Proxy key / placeholder are not forwarded upstream as user credentials.
    let api_key = match incoming_key {
        Some(k) if k == state.proxy_api_key || k == "minimax-no-key-required" => None,
        other => other,
    };

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
                            if let Some(name_val) =
                                inner_obj.get("name").or_else(|| obj.get("name")).or_else(|| obj.get("type"))
                            {
                                if let Some(name) = name_val.as_str() {
                                    valid_tools.push(models::ChatToolDefinition::Strict {
                                        r#type: "function".to_string(),
                                        function: models::FunctionDefinition {
                                            name: name.to_string(),
                                            description: inner_obj
                                                .get("description")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string()),
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

    if KiloProvider::is_kilo_model(&body.model) {
        match state.kilo_provider.post_chat(body).await {
            Ok(res) => relay_upstream_response(res).await,
            Err(e) => normalize_upstream_error(e),
        }
    } else if fallback::is_dead_upstream_model(&body.model) {
        // The original Blackbox upstreams for these model names are offline
        // (Cloud Run 404s). Re-route to verified-live equivalents instead of
        // returning a guaranteed-failure 502.
        match fallback::post_fallback(&state.kilo_provider, body).await {
            Ok(res) => relay_upstream_response(res).await,
            Err(e) => normalize_upstream_error(e),
        }
    } else {
        match state.provider.post_chat(api_key, body).await {
            Ok(res) => relay_upstream_response(res).await,
            Err(e) => normalize_upstream_error(e),
        }
    }
}

async fn handle_models() -> impl IntoResponse {
    let model_list = [
        ("minimax-m2.7", "minimax"),
        ("kimi-k2.6", "moonshot"),
        ("custom/blackbox-base-2", "blackbox"),
        ("gpt-4o-mini", "openai"),
        ("kilo-auto/free", "kilo"),
        ("openrouter/free", "openrouter"),
        ("poolside/laguna-m.1:free", "poolside"),
        ("stepfun/step-3.7-flash:free", "stepfun"),
        ("nvidia/nemotron-3-ultra-550b-a55b:free", "nvidia"),
        ("nex/nex-n2-pro:free", "nex"),
        ("inclusionai/ring-2.6-1t:free", "inclusionai"),
        ("inclusionai/ling-2.6-flash:free", "inclusionai"),
        ("google/gemma-4-26b-a4b:free", "google"),
        ("qwen/qwen3.8-27b:free", "qwen"),
        ("poolside/laguna-xs-2.1:free", "poolside"),
        ("poolside/laguna-s-2.1:free", "poolside"),
        ("liquid/lfm-2.5-2.6b:free", "liquid"),
        // Live fallback targets served under the original Blackbox names
        ("fallback-minimax", "nvidia"),
        ("fallback-kimi", "qwen"),
    ];
    let data: Vec<_> = model_list
        .iter()
        .map(|(id, owned_by)| {
            serde_json::json!({
                "id": id,
                "object": "model",
                "owned_by": owned_by,
                "free": true
            })
        })
        .collect();
    Json(serde_json::json!({ "object": "list", "data": data }))
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

pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/models", get(handle_models))
        .route("/docs", get(swagger_ui))
        .route("/docs/", get(swagger_ui))
        .route("/openapi.yaml", get(openapi_yaml))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    let state = Arc::new(AppState {
        provider: BlackboxProvider::new(client.clone()),
        kilo_provider: KiloProvider::new(client),
        proxy_api_key: env::var("PROXY_API_KEY").unwrap_or_else(|_| "xyz".to_string()),
    });

    let app = build_router(state);

    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();

    tracing::info!("🚀 Blackbox Provider running at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, Response};
    use tower::ServiceExt;

    #[test]
    fn extract_bearer_key_handles_common_forms() {
        assert_eq!(extract_bearer_key(Some("Bearer abc123")).as_deref(), Some("abc123"));
        assert_eq!(extract_bearer_key(Some("bearer abc123")).as_deref(), Some("abc123"));
        // Regression: a raw key that merely starts with the letters "bearer"
        // must keep its payload intact (only the scheme prefix is stripped).
        assert_eq!(extract_bearer_key(Some("bearkey-1")).as_deref(), Some("bearkey-1"));
        assert_eq!(extract_bearer_key(Some("  Bearer   spaced  ")).as_deref(), Some("spaced"));
        assert_eq!(extract_bearer_key(Some("rawkey")), Some("rawkey".to_string()));
        assert_eq!(extract_bearer_key(None), None);
        assert_eq!(extract_bearer_key(Some("")), None);
        assert_eq!(extract_bearer_key(Some("Bearer ")), None);
    }

    #[test]
    fn auth_accepts_exact_proxy_key_and_placeholder() {
        assert!(validate_proxy_auth(Some("xyz"), "xyz").is_ok());
        assert!(validate_proxy_auth(Some("minimax-no-key-required"), "xyz").is_ok());
        assert!(validate_proxy_auth(Some("anything"), "").is_ok()); // auth disabled
    }

    #[test]
    fn auth_rejects_missing_or_wrong_keys() {
        assert!(validate_proxy_auth(None, "xyz").is_err());
        assert!(validate_proxy_auth(Some("wrong"), "xyz").is_err());
        assert!(validate_proxy_auth(Some(""), "xyz").is_err());
    }

    #[test]
    fn auth_rejects_malformed_prefix_bypass_attempts() {
        // Regression: previously ANY key starting with "sk-"/"cus_" bypassed the gate.
        assert!(validate_proxy_auth(Some("sk-"), "xyz").is_err());
        assert!(validate_proxy_auth(Some("sk-x"), "xyz").is_err());
        assert!(validate_proxy_auth(Some("cus_"), "xyz").is_err());
        assert!(validate_proxy_auth(Some("sk-not a real key!"), "xyz").is_err());
    }

    #[test]
    fn auth_accepts_wellformed_upstream_credentials() {
        let sk = format!("sk-{}", "a".repeat(24));
        assert!(validate_proxy_auth(Some(&sk), "xyz").is_ok());
        let cus = format!("cus_{}", "b".repeat(16));
        assert!(validate_proxy_auth(Some(&cus), "xyz").is_ok());
    }

    #[tokio::test]
    async fn upstream_error_is_json_502() {
        let res = normalize_upstream_error("boom".to_string());
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["message"], "boom");
        assert_eq!(json["error"]["type"], "upstream_error");
    }

    #[tokio::test]
    async fn relay_passes_through_success_json() {
        let resp = Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"ok":true}"#.to_string())
                    .unwrap();
        let upstream = reqwest::Response::from(resp);
        let res = relay_upstream_response(upstream).await;
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), b"{\"ok\":true}");
    }

    #[tokio::test]
    async fn relay_normalizes_html_error_pages_to_json_502() {
        // Regression: dead Cloud Run backends returned HTML 404 pages which
        // were previously forwarded verbatim to clients.
        let resp = Response::builder()
                    .status(404)
                    .header("content-type", "text/html")
                    .body("<html><head></head><body>404 page not found</body></html>".to_string())
                    .unwrap();
        let upstream = reqwest::Response::from(resp);
        let res = relay_upstream_response(upstream).await;
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["type"], "upstream_error");
        let msg = json["error"]["message"].as_str().unwrap();
        assert!(msg.contains("404"), "message should mention upstream status: {msg}");
        assert!(!msg.contains("<html>"), "raw HTML must not leak into JSON message");
    }

    #[tokio::test]
    async fn relay_preserves_upstream_json_errors() {
        let resp = Response::builder()
                    .status(429)
                    .header("content-type", "application/json")
                    .body(r#"{"error":{"message":"rate limited"}}"#.to_string())
                    .unwrap();
        let upstream = reqwest::Response::from(resp);
        let res = relay_upstream_response(upstream).await;
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn models_endpoint_lists_all_models() {
        let state = Arc::new(AppState {
            provider: BlackboxProvider::new(reqwest::Client::new()),
            kilo_provider: KiloProvider::new(reqwest::Client::new()),
            proxy_api_key: "test-key".to_string(),
        });
        let app = build_router(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .method("GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["object"], "list");
        let data = json["data"].as_array().unwrap();
        assert_eq!(data.len(), 19);
        for m in crate::kilo::KILO_MODELS {
            assert!(
                data.iter().any(|d| d["id"] == *m),
                "model {m} missing from /v1/models"
            );
        }
    }

#[test]
    fn dead_upstream_aliases_are_recognized() {
        use crate::fallback::{dead_upstream_for, is_dead_upstream_model};
        for m in ["minimax-m2.7", "MINIMAX-M2", "gpt-5.5", "kimi-k2.6", "KIMI",
                  "moonshotai/kimi-k2.6", "gpt-4o-mini", "gpt-5.4-mini",
                  "custom/blackbox-base-2", "gpt-5.4"] {
            assert!(is_dead_upstream_model(m), "{m} should be a dead-upstream alias");
        }
        // Kilo-native and unknown models must NOT be captured by the table.
        assert!(!is_dead_upstream_model("kilo-auto/free"));
        assert!(!is_dead_upstream_model("some/unknown-model"));
        let e = dead_upstream_for("Kimi-K2.6").unwrap();
        assert_eq!(e.blackbox_name, "kimi-k2.6");
    }

    #[tokio::test]
    async fn dead_upstream_model_serves_fallback_completion_end_to_end() {
        use crate::fallback;
        // Mock "live Kilo gateway": returns a valid completion for any model.
        let kilo_json = r#"{"id":"c","object":"chat.completion","created":1,"model":"stepfun/step-3.7-flash:free","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}"#.to_string();
        let mock = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let body = kilo_json.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });

        let client = reqwest::Client::new();
        let kilo = KiloProvider::new_at(client.clone(), format!("http://{}/", addr));

        // Every dead Blackbox alias must resolve to a live replacement that
        // is itself routable through the Kilo provider...
        for m in ["minimax-m2.7", "kimi-k2.6", "gpt-4o-mini", "custom/blackbox-base-2"] {
            let entry = fallback::dead_upstream_for(m).expect("alias mapped");
            for r in entry.replacements {
                assert!(
                    KiloProvider::is_kilo_model(r),
                    "replacement {r} for {m} must be routable through the Kilo provider"
                );
            }
        }

        // ...and the full fallback path serves a real completion end-to-end.
        let body = serde_json::from_value::<crate::models::ChatCompletionRequest>(
            serde_json::json!({
                "model": "minimax-m2.7",
                "messages": [{"role": "user", "content": "hi"}]
            }),
        )
        .unwrap();
        let res = fallback::post_fallback(&kilo, body).await.expect("fallback succeeds");
        assert_eq!(res.status(), StatusCode::OK);
        let relayed = relay_upstream_response(res).await;
        assert_eq!(relayed.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(relayed.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["choices"][0]["message"]["content"], "OK");

        // The relay of a mocked dead-Blackbox HTML 404 yields a JSON 502
        // (used when every fallback also fails), never raw HTML.
        let resp = http::Response::builder()
            .status(404)
            .header("content-type", "text/html")
            .body("<html><body>404 page not found</body></html>".to_string())
            .unwrap();
        let res = relay_upstream_response(reqwest::Response::from(resp)).await;
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("error body must be JSON");
    }

    #[tokio::test]
    async fn chat_completions_rejects_bad_key_with_structured_401() {
        let state = Arc::new(AppState {
            provider: BlackboxProvider::new(reqwest::Client::new()),
            kilo_provider: KiloProvider::new(reqwest::Client::new()),
            proxy_api_key: "test-key".to_string(),
        });
        let app = build_router(state);
        let body = serde_json::json!({
            "model": "minimax-m2.7",
            "messages": [{"role": "user", "content": "hi"}]
        });
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("authorization", "Bearer totally-wrong")
                    .header("content-type", "application/json")
                    .body(body.to_string())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let bytes = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "invalid_api_key");
    }
}
