//! Live fallback routing for models whose original upstreams are dead.
//!
//! The Blackbox free/pro Cloud Run backends that used to serve
//! `minimax-m2.7`, `kimi-k2.6`, `gpt-4o-mini` and `custom/blackbox-base-2`
//! have been shut down (they return HTTP 404 HTML for every path, verified
//! live). Rather than failing those model names with a 502 forever, the proxy
//! now maps them onto *live* equivalents on the Kilo gateway's anonymous
//! free tier. Each alias is tried first; if it fails, the request continues
//! through the shared KILO_MODELS failover pool via `KiloProvider::post_chat`.

use crate::kilo::KiloProvider;
use crate::models::ChatCompletionRequest;

/// Maps a dead Blackbox model name to a list of live replacement model ids
/// (tried in order), plus a human-readable reason surfaced in logs.
pub struct DeadUpstream {
    pub blackbox_name: &'static str,
    pub replacements: &'static [&'static str],
    pub reason: &'static str,
}

/// Canonical aliases clients may use for each dead Blackbox model.
const MINIMAX_ALIASES: &[&str] = &["minimax-m2.7", "minimax-m2", "minimax-m2.5", "gpt-5.5", "5.5"];
const KIMI_ALIASES: &[&str] = &["kimi-k2.6", "kimi", "moonshotai/kimi-k2.6"];
const GPT4O_MINI_ALIASES: &[&str] = &["gpt-4o-mini", "gpt-5.4-mini", "5.4 mini", "5.4-mini"];
const BLACKBOX_BASE_ALIASES: &[&str] = &["custom/blackbox-base-2", "blackbox-base-2", "gpt-5.4", "5.4"];

/// The full table of dead-upstream aliases and their live replacements.
/// Replacement ids must be members of `KILO_MODELS` so they can ride the
/// existing anonymous free-tier route.
pub const DEAD_UPSTREAMS: &[DeadUpstream] = &[
    DeadUpstream {
        // Big reasoning model; the Kilo failover pool (nemotron-ultra, etc.)
        // backs this alias when stepfun is rate-limited.
        blackbox_name: "minimax-m2.7",
        replacements: &["stepfun/step-3.7-flash:free"],
        reason: "Blackbox free/pro Cloud Run endpoints are offline (HTTP 404)",
    },
    DeadUpstream {
        blackbox_name: "kimi-k2.6",
        replacements: &["qwen/qwen3.8-27b:free"],
        reason: "Blackbox Kimi route required a customer token against now-offline endpoints",
    },
    DeadUpstream {
        blackbox_name: "gpt-4o-mini",
        replacements: &["poolside/laguna-xs-2.1:free", "liquid/lfm-2.5-2.6b:free"],
        reason: "Blackbox gpt-4o-mini route pointed at offline Cloud Run endpoints",
    },
    DeadUpstream {
        blackbox_name: "custom/blackbox-base-2",
        replacements: &["poolside/laguna-s-2.1:free", "kilo-auto/free"],
        reason: "Blackbox base model route pointed at offline Cloud Run endpoints",
    },
];

fn matches_any(lowered: &str, aliases: &[&str]) -> bool {
    aliases.iter().any(|a| *a == lowered)
}

/// Returns the `DeadUpstream` entry whose alias list contains `model_id`
/// (case-insensitively), if any.
pub fn dead_upstream_for(model_id: &str) -> Option<&'static DeadUpstream> {
    let lower = model_id.to_lowercase();
    DEAD_UPSTREAMS.iter().find(|d| {
        d.blackbox_name.to_lowercase() == lower
            || matches_any(
                &lower,
                match d.blackbox_name {
                    "minimax-m2.7" => MINIMAX_ALIASES,
                    "kimi-k2.6" => KIMI_ALIASES,
                    "gpt-4o-mini" => GPT4O_MINI_ALIASES,
                    _ => BLACKBOX_BASE_ALIASES,
                },
            )
    })
}

/// True when the requested model is a known alias for a dead Blackbox route
/// and therefore eligible for fallback re-routing.
pub fn is_dead_upstream_model(model_id: &str) -> bool {
    dead_upstream_for(model_id).is_some()
}

/// Post a chat completion originally destined for a dead Blackbox upstream,
/// trying each live replacement model in order. Each attempt goes through
/// `KiloProvider::post_chat`, so the shared KILO_MODELS failover pool backs
/// every alias as well.
pub async fn post_fallback(
    kilo: &KiloProvider,
    body: ChatCompletionRequest,
) -> Result<reqwest::Response, String> {
    let entry = dead_upstream_for(&body.model).ok_or_else(|| {
        format!("No fallback configured for model '{}'", body.model)
    })?;

    tracing::warn!(
        "Model '{}' upstream is dead ({}); falling back to live replacements {:?}",
        entry.blackbox_name,
        entry.reason,
        entry.replacements
    );

    let mut last_err = String::new();
    for replacement in entry.replacements {
        let mut req_body = body.clone();
        req_body.model = replacement.to_string();
        match kilo.post_chat(req_body).await {
            Ok(res) => return Ok(res),
            Err(e) => {
                tracing::warn!("Fallback '{}' failed: {}", replacement, e);
                last_err = e;
            }
        }
    }
    Err(format!(
        "All fallback models for '{}' are unavailable (original upstream: {}). Last error: {}",
        entry.blackbox_name, entry.reason, last_err
    ))
}
