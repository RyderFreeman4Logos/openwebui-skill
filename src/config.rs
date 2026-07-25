
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_TIMEOUT: u64 = 3300;
const DEFAULT_POLL_INTERVAL: u64 = 3;
const DEFAULT_CONNECT_TIMEOUT: u64 = 10;
const DEFAULT_REQUEST_TIMEOUT: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT
}

fn default_poll_interval() -> u64 {
    DEFAULT_POLL_INTERVAL
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            default_model: String::new(),
            timeout: DEFAULT_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TomlConfig {
    base_url: Option<String>,
    api_key: Option<String>,
    default_model: Option<String>,
    timeout: Option<u64>,
    poll_interval: Option<u64>,
}

impl Config {
    /// Load configuration with CLI override priority.
    ///
    /// Priority (highest first):
    /// 1. CLI flags
    /// 2. Environment variables
    /// 3. TOML config file
    /// 4. Built-in defaults
    pub fn load(
        cli_base_url: Option<&str>,
        cli_api_key: Option<&str>,
        cli_model: Option<&str>,
        cli_timeout: Option<u64>,
        cli_poll_interval: Option<u64>,
    ) -> Result<Self> {
        // Load .env file if present (best-effort)
        let _ = dotenvy::dotenv();

        // Try TOML config files
        let toml_cfg = Self::load_toml()?;

        // Environment variables
        let env_base_url = std::env::var("OPENWEBUI_BASE_URL").ok();
        let env_api_key = std::env::var("OPENWEBUI_API_KEY").ok();
        let env_model = std::env::var("OPENWEBUI_DEFAULT_MODEL").ok();
        let env_timeout = std::env::var("OPENWEBUI_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());

        // Resolve with priority: CLI > env > toml > default
        let base_url = cli_base_url
            .map(String::from)
            .or(env_base_url)
            .or(toml_cfg.as_ref().and_then(|c| c.base_url.clone()))
            .ok_or_else(|| {
                anyhow!(
                    "No base URL configured. Set OPENWEBUI_BASE_URL or use --base-url, \
                     or add 'base_url' to ~/.config/openwebui-chat/config.toml"
                )
            })?;

        let api_key = cli_api_key
            .map(String::from)
            .or(env_api_key)
            .or(toml_cfg.as_ref().and_then(|c| c.api_key.clone()))
            .ok_or_else(|| {
                anyhow!(
                    "No API key configured. Set OPENWEBUI_API_KEY or use --api-key, \
                     or add 'api_key' to ~/.config/openwebui-chat/config.toml"
                )
            })?;

        let default_model = cli_model
            .map(String::from)
            .or(env_model)
            .or(toml_cfg.as_ref().and_then(|c| c.default_model.clone()))
            .unwrap_or_default();

        let timeout = cli_timeout
            .or(env_timeout)
            .or(toml_cfg.as_ref().and_then(|c| c.timeout))
            .unwrap_or(DEFAULT_TIMEOUT);

        let poll_interval = cli_poll_interval
            .or(toml_cfg.as_ref().and_then(|c| c.poll_interval))
            .unwrap_or(DEFAULT_POLL_INTERVAL);

        Ok(Config {
            base_url,
            api_key,
            default_model,
            timeout,
            poll_interval,
        })
    }

    fn load_toml() -> Result<Option<TomlConfig>> {
        // Try user config dir first, then local directory
        let candidates = [
            dirs::config_dir().map(|d| d.join("openwebui-chat").join("config.toml")),
            Some(std::path::PathBuf::from("openwebui-chat.toml")),
        ];

        for candidate in candidates.iter().flatten() {
            if candidate.exists() {
                let content = std::fs::read_to_string(candidate)
                    .with_context(|| format!("Failed to read config file: {}", candidate.display()))?;
                let cfg: TomlConfig = toml::from_str(&content)
                    .with_context(|| format!("Failed to parse config file: {}", candidate.display()))?;
                return Ok(Some(cfg));
            }
        }

        Ok(None)
    }

    pub fn http_client(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(DEFAULT_CONNECT_TIMEOUT))
            .timeout(std::time::Duration::from_secs(DEFAULT_REQUEST_TIMEOUT))
            .build()
            .context("Failed to create HTTP client")
    }

    /// Required API endpoints for the Open WebUI API key.
    /// These should be added to the `auth.api_key.allowed_endpoints` config.
    pub fn required_endpoints() -> Vec<&'static str> {
        vec![
            "/api/chat/completions",
            "/api/v1/chats",
            "/api/v1/chats/new",
            "/api/v1/chats/{id}",
        ]
    }

}
