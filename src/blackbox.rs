use std::env;
use std::time::Duration;

use reqwest::{Client, header};
use tokio::time::timeout;

use crate::models::{ChatCompletionRequest, ChatCompletionResponse};
use crate::vscode::extract_vscode_blackbox_tokens;
use crate::utils::random_id;

const BLACKBOX_API_BASE: &str = "https://api.blackbox.ai/v1";
const BLACKBOX_FREE_BASE: &str = "https://oi-vscode-server-985058387028.europe-west1.run.app";
const BLACKBOX_PRO_BASE: &str = "https://oi-vscode-server-pro-985058387028.europe-west1.run.app";

pub struct BlackboxProvider {
    client: Client,
}

impl BlackboxProvider {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn is_placeholder_key(key: Option<&String>) -> bool {
        match key {
            Some(k) => {
                let trimmed = k.trim();
                trimmed.is_empty() 
                    || trimmed == "xxx" 
                    || trimmed == "minimax-no-key-required"
            },
            None => true,
        }
    }

    fn get_env_api_key() -> Option<String> {
        let val = env::var("BLACKBOX_API_KEY")
            .or_else(|_| env::var("BLACKBOX_CUSTOMER_ID"))
            .or_else(|_| env::var("BLACKBOX_CUSTOMER_TOKEN"))
            .ok();
        
        if Self::is_placeholder_key(val.as_ref()) {
            None
        } else {
            val.map(|s| s.trim().to_string())
        }
    }

    fn normalize_model_id(model_id: &str) -> String {
        let lower = model_id.to_lowercase();
        if lower == "kimi-k2.6" || lower == "kimi" {
            return "moonshotai/kimi-k2.6".to_string();
        }
        if lower == "gpt-5.5" || lower == "5.5" {
            return "minimax-m2.7".to_string();
        }
        if lower == "gpt-5.4" || lower == "5.4" {
            return "custom/blackbox-base-2".to_string();
        }
        if lower == "gpt-5.4-mini" || lower == "5.4 mini" || lower == "5.4-mini" {
            return "gpt-4o-mini".to_string();
        }
        model_id.to_string()
    }

    fn is_minimax_model(model_id: &str) -> bool {
        let lower = model_id.to_lowercase();
        lower == "minimax-m2"
            || lower == "minimax-m2.7"
            || lower == "minimax-m2.5"
            || lower == "minimax-free"
            || lower == "openrouter/minimax-m2-thinking"
    }

    fn is_kimi_model(model_id: &str) -> bool {
        model_id.to_lowercase() == "moonshotai/kimi-k2.6"
    }

    pub async fn post_chat(
        &self,
        api_key: Option<String>,
        mut body: ChatCompletionRequest,
    ) -> Result<reqwest::Response, String> {
        let model_id = Self::normalize_model_id(&body.model);
        let is_minimax = Self::is_minimax_model(&model_id);
        let is_kimi = Self::is_kimi_model(&model_id);
        
        body.model = if is_minimax {
            "openrouter/minimax-m2-thinking".to_string()
        } else {
            model_id.clone()
        };

        let mut effective_api_key = if Self::is_placeholder_key(api_key.as_ref()) {
            None
        } else {
            api_key.map(|k| k.trim().to_string())
        };

        if effective_api_key.is_none() {
            effective_api_key = Self::get_env_api_key();
        }

        if effective_api_key.is_none() {
            let tokens = extract_vscode_blackbox_tokens();
            if tokens.customer_id.is_some() {
                effective_api_key = tokens.customer_id;
            } else if tokens.api_key.is_some() {
                effective_api_key = tokens.api_key;
            }
        }

        if effective_api_key.is_none() && is_minimax {
            effective_api_key = Some("minimax-no-key-required".to_string());
        }

        if effective_api_key.is_none() && is_kimi {
            return Err("Blackbox Kimi K2.6 requires a saved Blackbox customer token. Open the Blackbox extension/sidebar and send a normal chat message first, or pass a valid Blackbox API key.".to_string());
        }

        if let Some(key) = &effective_api_key {
            if key.starts_with("sk-") {
                let url = format!("{}/chat/completions", BLACKBOX_API_BASE);
                let req = self.client.post(&url)
                    .header(header::AUTHORIZATION, format!("Bearer {}", key))
                    .header(header::CONTENT_TYPE, "application/json")
                    .json(&body);
                
                return req.send().await.map_err(|e| e.to_string());
            }
        }

        let user_id = random_id(16);
        let is_customer_token = match &effective_api_key {
            Some(k) if k != "minimax-no-key-required" && k.len() > 10 => true,
            _ => false,
        };

        let auth_val = if let Some(k) = &effective_api_key {
            if k == "minimax-no-key-required" {
                k.clone()
            } else {
                "xxx".to_string()
            }
        } else {
            "xxx".to_string()
        };

        let target_base = if is_customer_token { BLACKBOX_PRO_BASE } else { BLACKBOX_FREE_BASE };
        let url = format!("{}/chat/completions", target_base);
        
        let mut req = self.client.post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {}", auth_val))
            .header("userId", user_id)
            .header("version", "1.1");
            
        if is_customer_token {
            if let Some(key) = &effective_api_key {
                req = req.header("customerId", key);
            }
        }

        req.json(&body).send().await.map_err(|e| e.to_string())
    }
}
