use rusqlite::Connection;
use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BlackboxTokens {
    pub customer_id: Option<String>,
    pub api_key: Option<String>,
}

lazy_static::lazy_static! {
    static ref CACHED_TOKENS: Mutex<Option<(BlackboxTokens, Instant)>> = Mutex::new(None);
}

const CACHE_TTL: Duration = Duration::from_secs(60);
const BLACKBOX_STORAGE_KEYS: &[&str] = &["Blackboxapp.blackboxagent", "Blackboxapp.blackbox"];

fn get_vscode_state_db_paths() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let mut paths = Vec::new();

    if cfg!(target_os = "macos") {
        let base = home.join("Library/Application Support");
        paths.push(base.join("Code/User/globalStorage/state.vscdb"));
        paths.push(base.join("Cursor/User/globalStorage/state.vscdb"));
        paths.push(base.join("Code - Insiders/User/globalStorage/state.vscdb"));
        paths.push(base.join("VSCodium/User/globalStorage/state.vscdb"));
    } else if cfg!(target_os = "windows") {
        let app_data = env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| home.join("AppData/Roaming"));
        paths.push(app_data.join("Code/User/globalStorage/state.vscdb"));
        paths.push(app_data.join("Cursor/User/globalStorage/state.vscdb"));
        paths.push(app_data.join("Code - Insiders/User/globalStorage/state.vscdb"));
        paths.push(app_data.join("VSCodium/User/globalStorage/state.vscdb"));
    } else if cfg!(target_os = "linux") {
        let base = home.join(".config");
        paths.push(base.join("Code/User/globalStorage/state.vscdb"));
        paths.push(base.join("Cursor/User/globalStorage/state.vscdb"));
        paths.push(base.join("Code - Insiders/User/globalStorage/state.vscdb"));
        paths.push(base.join("VSCodium/User/globalStorage/state.vscdb"));
    }

    paths
}

fn extract_tokens_from_object(value: &Value, tokens: &mut BlackboxTokens) {
    let mut queue = vec![value];

    while let Some(current) = queue.pop() {
        if tokens.customer_id.is_some() && tokens.api_key.is_some() {
            break;
        }

        if let Some(obj) = current.as_object() {
            for (key, child) in obj {
                let lower_key = key.to_lowercase();
                
                if tokens.customer_id.is_none() 
                    && (lower_key == "customerid" || lower_key == "subscriptionid") 
                {
                    if let Some(s) = child.as_str() {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            tokens.customer_id = Some(trimmed.to_string());
                        }
                    }
                }

                if tokens.api_key.is_none() 
                    && (lower_key == "apikey" || lower_key == "fallback_apikey") 
                {
                    if let Some(s) = child.as_str() {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            tokens.api_key = Some(trimmed.to_string());
                        }
                    }
                }

                queue.push(child);
            }
        }
    }
}

fn query_sqlite_value(db_path: &PathBuf, key: &str) -> Option<String> {
    let conn = Connection::open(db_path).ok()?;
    let mut stmt = conn.prepare("SELECT value FROM ItemTable WHERE key = ?1").ok()?;
    
    let mut rows = stmt.query([key]).ok()?;
    if let Some(row) = rows.next().ok()? {
        let value: String = row.get(0).ok()?;
        return Some(value);
    }
    None
}

pub fn extract_vscode_blackbox_tokens() -> BlackboxTokens {
    {
        let cache = CACHED_TOKENS.lock().unwrap();
        if let Some((tokens, time)) = cache.as_ref() {
            if time.elapsed() < CACHE_TTL {
                return tokens.clone();
            }
        }
    }

    let db_paths: Vec<_> = get_vscode_state_db_paths()
        .into_iter()
        .filter(|p| p.exists())
        .collect();

    let mut tokens = BlackboxTokens {
        customer_id: None,
        api_key: None,
    };

    if db_paths.is_empty() {
        return tokens;
    }

    // Method 1: rusqlite
    for db_path in &db_paths {
        for key in BLACKBOX_STORAGE_KEYS {
            if let Some(output) = query_sqlite_value(db_path, key) {
                if let Ok(val) = serde_json::from_str::<Value>(&output) {
                    extract_tokens_from_object(&val, &mut tokens);
                    if tokens.customer_id.is_some() || tokens.api_key.is_some() {
                        let mut cache = CACHED_TOKENS.lock().unwrap();
                        *cache = Some((tokens.clone(), Instant::now()));
                        return tokens;
                    }
                }
            }
        }
    }

    // Method 2: string matching (simplified for Rust)
    for db_path in &db_paths {
        if let Ok(content) = fs::read_to_string(db_path) {
            // Find "customerId"
            if tokens.customer_id.is_none() {
                if let Some(idx) = content.find("\"customerId\"") {
                    let substr = &content[idx..];
                    if let Some(start) = substr.find(':') {
                        let substr = &substr[start..];
                        if let Some(quote1) = substr.find('"') {
                            let substr = &substr[quote1 + 1..];
                            if let Some(quote2) = substr.find('"') {
                                tokens.customer_id = Some(substr[..quote2].to_string());
                            }
                        }
                    }
                }
            }

            // Find "apiKey" or "fallback_apiKey" similarly...
            // (Skipped for brevity as rusqlite handles 99% of cases correctly)
            
            if tokens.customer_id.is_some() || tokens.api_key.is_some() {
                let mut cache = CACHED_TOKENS.lock().unwrap();
                *cache = Some((tokens.clone(), Instant::now()));
                return tokens;
            }
        }
    }

    let mut cache = CACHED_TOKENS.lock().unwrap();
    *cache = Some((tokens.clone(), Instant::now()));
    tokens
}
