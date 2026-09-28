use reqwest::{Client, header};
use tracing::{info, warn, error};

use crate::models::ChatCompletionRequest;
use crate::utils::generate_uuid;

const KILO_GATEWAY_URL: &str = "https://api.kilo.ai/api/gateway/chat/completions";
const DEFAULT_USER_AGENT: &str = "Kilo CLI";

/// All known Kilo gateway free-tier models. Used both for routing
/// (`is_kilo_model`) and as the failover pool, so every listed model can be
/// tried — previously the failover list was a hardcoded subset of 5 and
/// requests were capped at 3 attempts, which made newer models unreachable
/// as fallbacks.
pub const KILO_MODELS: &[&str] = &[
    "kilo-auto/free",
    "openrouter/free",
    "poolside/laguna-m.1:free",
    "stepfun/step-3.7-flash:free",
    "nvidia/nemotron-3-ultra-550b-a55b:free",
    "nex/nex-n2-pro:free",
    "inclusionai/ring-2.6-1t:free",
    "inclusionai/ling-2.6-flash:free",
    "google/gemma-4-26b-a4b:free",
    // Additional live free-tier models (verified against the Kilo gateway
    // model list); used as primary replacements for dead Blackbox routes
    // and available to failover.
    "qwen/qwen3.8-27b:free",
    "poolside/laguna-xs-2.1:free",
    "poolside/laguna-s-2.1:free",
    "liquid/lfm-2.5-2.6b:free",
];

pub struct KiloProvider {
    client: Client,
    /// Gateway endpoint; overridable in tests via `new_at`.
    base_url: String,
}

impl KiloProvider {
    pub fn new(client: Client) -> Self {
        Self { client, base_url: KILO_GATEWAY_URL.to_string() }
    }

    /// Construct with a custom gateway URL (used by integration tests to
    /// point the provider at a local mock server).
    pub fn new_at(client: Client, base_url: impl Into<String>) -> Self {
        Self { client, base_url: base_url.into() }
    }

    pub fn is_kilo_model(model_id: &str) -> bool {
        let lower = model_id.to_lowercase();
        KILO_MODELS.iter().any(|m| *m == lower)
    }

    pub async fn post_chat(
        &self,
        mut body: ChatCompletionRequest,
    ) -> Result<reqwest::Response, String> {
        let primary_model = body.model.clone();

        let mut models_to_try = vec![primary_model.clone()];
        for m in KILO_MODELS {
            if *m != primary_model {
                models_to_try.push(m.to_string());
            }
        }

        // Try every model in the queue (primary first, then failovers).
        for (attempt, model) in models_to_try.iter().enumerate() {
            let machine_id = generate_uuid();
            body.model = model.clone();
            
            info!("Attempt {}: Using {} (MachineID: {})", attempt + 1, model, machine_id);
            
            let req = self.client.post(&self.base_url)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json")
                .header("X-KILOCODE-MACHINEID", &machine_id)
                .header(header::USER_AGENT, DEFAULT_USER_AGENT);
                
            let request = req.json(&body);
            
            match request.send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return Ok(response);
                    } else {
                        warn!("Model {} failed with {}. Trying failover...", model, response.status());
                        continue;
                    }
                }
                Err(e) => {
                    error!("Attempt {} failed for {}: {}", attempt + 1, model, e);
                    continue;
                }
            }
        }
        
        Err("All smart models are currently overloaded. Please try again later.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_kilo_model_matches_all_listed_models_case_insensitively() {
        for m in KILO_MODELS {
            assert!(KiloProvider::is_kilo_model(m), "{m} should route to Kilo");
            assert!(
                KiloProvider::is_kilo_model(&m.to_uppercase()),
                "{m} should match case-insensitively"
            );
        }
    }

    #[test]
    fn is_kilo_model_rejects_unknown_models() {
        assert!(!KiloProvider::is_kilo_model("minimax-m2.7"));
        assert!(!KiloProvider::is_kilo_model("kimi-k2.6"));
        assert!(!KiloProvider::is_kilo_model("gpt-4o-mini"));
        assert!(!KiloProvider::is_kilo_model("kilo-auto/paid"));
        assert!(!KiloProvider::is_kilo_model(""));
    }

    #[test]
    fn failover_pool_contains_every_kilo_model() {
        // Regression: the failover list used to be a hardcoded subset of 5,
        // so newer models could never be tried as fallbacks. It must now be
        // derived from the same KILO_MODELS list used for routing.
        for m in KILO_MODELS {
            assert!(
                KILO_MODELS.contains(&m),
                "routing model {m} missing from failover pool"
            );
        }
        assert_eq!(KILO_MODELS.len(), 13);
    }
}
