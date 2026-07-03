use reqwest::{Client, header};
use tracing::{info, warn, error};

use crate::models::ChatCompletionRequest;
use crate::utils::generate_uuid;

const KILO_GATEWAY_URL: &str = "https://api.kilo.ai/api/gateway/chat/completions";
const DEFAULT_USER_AGENT: &str = "Kilo CLI";

const FAILOVER_MODELS: &[&str] = &[
    "kilo-auto/free",
    "openrouter/free",
    "poolside/laguna-m.1:free",
    "stepfun/step-3.7-flash:free",
    "nvidia/nemotron-3-ultra-550b-a55b:free",
];

pub struct KiloProvider {
    client: Client,
}

impl KiloProvider {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub fn is_kilo_model(model_id: &str) -> bool {
        let lower = model_id.to_lowercase();
        // Return true if it matches any known Kilo free models
        lower == "kilo-auto/free"
            || lower == "openrouter/free"
            || lower == "poolside/laguna-m.1:free"
            || lower == "stepfun/step-3.7-flash:free"
            || lower == "nvidia/nemotron-3-ultra-550b-a55b:free"
            || lower == "nex/nex-n2-pro:free"
            || lower == "inclusionai/ring-2.6-1t:free"
            || lower == "inclusionai/ling-2.6-flash:free"
            || lower == "google/gemma-4-26b-a4b:free"
    }

    pub async fn post_chat(
        &self,
        mut body: ChatCompletionRequest,
    ) -> Result<reqwest::Response, String> {
        let primary_model = body.model.clone();
        
        let mut models_to_try = vec![primary_model.clone()];
        for m in FAILOVER_MODELS {
            if *m != primary_model {
                models_to_try.push(m.to_string());
            }
        }

        // Try up to 3 models in the queue
        for (attempt, model) in models_to_try.iter().take(3).enumerate() {
            let machine_id = generate_uuid();
            body.model = model.clone();
            
            info!("Attempt {}: Using {} (MachineID: {})", attempt + 1, model, machine_id);
            
            let req = self.client.post(KILO_GATEWAY_URL)
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
