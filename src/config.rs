use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_TIMEOUT: u64 = 3300;
const DEFAULT_POLL_INTERVAL: u64 = 3;
const DEFAULT_WEB_SEARCH: bool = true;
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
    #[serde(default = "default_web_search")]
    pub web_search: bool,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT
}

fn default_poll_interval() -> u64 {
    DEFAULT_POLL_INTERVAL
}

fn default_web_search() -> bool {
    DEFAULT_WEB_SEARCH
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            default_model: String::new(),
            timeout: DEFAULT_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            web_search: DEFAULT_WEB_SEARCH,
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
    web_search: Option<bool>,
}

impl Config {
    /// Return the XDG config file path.
    pub fn config_path() -> Result<std::path::PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not determine config directory"))?
            .join("openwebui-chat");
        Ok(config_dir.join("config.toml"))
    }

    /// Write the current config to the XDG config file.
    pub fn save_to_xdg(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }
        let toml_string =
            toml::to_string_pretty(self).context("Failed to serialize config to TOML")?;
        std::fs::write(&path, toml_string)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;
        Ok(())
    }

    /// Load only the XDG config file without applying CLI, environment, or local-file overrides.
    pub(crate) fn load_xdg() -> Result<Option<Self>> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(None);
        }

        let config = Self::read_toml_config(&path)?;
        Ok(Some(Self {
            base_url: config.base_url.unwrap_or_default(),
            api_key: config.api_key.unwrap_or_default(),
            default_model: config.default_model.unwrap_or_default(),
            timeout: config.timeout.unwrap_or(DEFAULT_TIMEOUT),
            poll_interval: config.poll_interval.unwrap_or(DEFAULT_POLL_INTERVAL),
            web_search: config.web_search.unwrap_or(DEFAULT_WEB_SEARCH),
        }))
    }

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

        let web_search = toml_cfg
            .as_ref()
            .and_then(|c| c.web_search)
            .unwrap_or(DEFAULT_WEB_SEARCH);

        Ok(Config {
            base_url,
            api_key,
            default_model,
            timeout,
            poll_interval,
            web_search,
        })
    }

    fn load_toml() -> Result<Option<TomlConfig>> {
        let mut candidates = Vec::new();

        // Primary XDG path.
        if let Ok(path) = Self::config_path() {
            candidates.push(path);
        }
        // Local project configuration.
        candidates.push(std::path::PathBuf::from("openwebui-chat.toml"));
        // HOME can differ between installation and runtime processes.
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(
                std::path::PathBuf::from(home)
                    .join(".config")
                    .join("openwebui-chat")
                    .join("config.toml"),
            );
        }
        // Common persistent configuration location for containerized installs.
        candidates.push(std::path::PathBuf::from(
            "/opt/data/.config/openwebui-chat/config.toml",
        ));

        for candidate in candidates {
            if candidate.exists() {
                return Self::read_toml_config(&candidate).map(Some);
            }
        }

        Ok(None)
    }

    fn read_toml_config(path: &std::path::Path) -> Result<TomlConfig> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
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
            // Used for both waiting on a chat (GET) and deleting a session (DELETE).
            "/api/v1/chats/{id}",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::sync::Mutex;

    static CONFIG_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn web_search_defaults_to_enabled_and_can_be_disabled_in_toml() -> anyhow::Result<()> {
        let enabled: Config = toml::from_str(
            "base_url = \"http://example.test:8080\"\napi_key = \"test-api-key\"\n",
        )?;
        assert!(enabled.web_search);

        let disabled: Config = toml::from_str(
            "base_url = \"http://example.test:8080\"\napi_key = \"test-api-key\"\nweb_search = false\n",
        )?;
        assert!(!disabled.web_search);
        Ok(())
    }

    #[test]
    fn save_to_xdg_writes_config_toml_to_the_xdg_directory() -> anyhow::Result<()> {
        let _lock = CONFIG_ENV_LOCK.lock().unwrap();
        let temp_dir =
            std::env::temp_dir().join(format!("openwebui-chat-test-{}", uuid::Uuid::new_v4()));
        let previous_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &temp_dir);

        let config = Config {
            base_url: "http://example.test:8080".to_string(),
            api_key: "test-api-key".to_string(),
            default_model: "test-model".to_string(),
            timeout: 45,
            poll_interval: 2,
            web_search: true,
        };

        let result = (|| -> anyhow::Result<()> {
            config.save_to_xdg()?;

            let path = temp_dir.join("openwebui-chat").join("config.toml");
            let saved: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
            assert_eq!(saved["base_url"].as_str(), Some("http://example.test:8080"));
            assert_eq!(saved["api_key"].as_str(), Some("test-api-key"));
            assert_eq!(saved["default_model"].as_str(), Some("test-model"));
            assert_eq!(saved["timeout"].as_integer(), Some(45));
            assert_eq!(saved["poll_interval"].as_integer(), Some(2));
            assert_eq!(saved["web_search"].as_bool(), Some(true));
            Ok(())
        })();

        match previous_xdg_config_home {
            Some(path) => std::env::set_var("XDG_CONFIG_HOME", path),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&temp_dir);

        result
    }

    #[test]
    fn load_xdg_uses_saved_values_without_environment_overrides() -> anyhow::Result<()> {
        let _lock = CONFIG_ENV_LOCK.lock().unwrap();
        let temp_dir =
            std::env::temp_dir().join(format!("openwebui-chat-test-{}", uuid::Uuid::new_v4()));
        let previous_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");
        let previous_base_url = std::env::var_os("OPENWEBUI_BASE_URL");
        std::env::set_var("XDG_CONFIG_HOME", &temp_dir);

        let config = Config {
            base_url: "http://saved.example.test:8080".to_string(),
            api_key: "saved-api-key".to_string(),
            default_model: "saved-model".to_string(),
            timeout: 45,
            poll_interval: 2,
            web_search: false,
        };

        let result = (|| -> anyhow::Result<()> {
            config.save_to_xdg()?;
            std::env::set_var("OPENWEBUI_BASE_URL", "http://environment.example.test:8080");

            let loaded = Config::load_xdg()?.expect("config should have been saved");
            assert_eq!(loaded.base_url, config.base_url);
            assert_eq!(loaded.api_key, config.api_key);
            assert_eq!(loaded.default_model, config.default_model);
            assert_eq!(loaded.timeout, config.timeout);
            assert_eq!(loaded.poll_interval, config.poll_interval);
            assert_eq!(loaded.web_search, config.web_search);
            Ok(())
        })();

        match previous_xdg_config_home {
            Some(path) => std::env::set_var("XDG_CONFIG_HOME", path),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match previous_base_url {
            Some(value) => std::env::set_var("OPENWEBUI_BASE_URL", value),
            None => std::env::remove_var("OPENWEBUI_BASE_URL"),
        }
        let _ = std::fs::remove_dir_all(&temp_dir);

        result
    }

    #[test]
    fn load_toml_uses_home_based_fallback_when_xdg_config_is_missing() -> anyhow::Result<()> {
        let _lock = CONFIG_ENV_LOCK.lock().unwrap();
        let temp_dir =
            std::env::temp_dir().join(format!("openwebui-chat-test-{}", uuid::Uuid::new_v4()));
        let xdg_config_home = temp_dir.join("xdg");
        let home = temp_dir.join("home");
        let cwd = temp_dir.join("cwd");
        let home_config = home.join(".config/openwebui-chat/config.toml");
        let previous_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");
        let previous_home = std::env::var_os("HOME");
        let previous_cwd = std::env::current_dir()?;

        let result = (|| -> anyhow::Result<()> {
            std::fs::create_dir_all(home_config.parent().expect("config path has a parent"))?;
            std::fs::create_dir_all(&cwd)?;
            std::fs::write(
                &home_config,
                "base_url = \"http://home-config.example.test:8080\"\napi_key = \"home-api-key\"\n",
            )?;
            std::env::set_var("XDG_CONFIG_HOME", &xdg_config_home);
            std::env::set_var("HOME", &home);
            std::env::set_current_dir(&cwd)?;

            let config = Config::load_toml()?.expect("HOME fallback config should be loaded");
            assert_eq!(
                config.base_url.as_deref(),
                Some("http://home-config.example.test:8080")
            );
            assert_eq!(config.api_key.as_deref(), Some("home-api-key"));
            Ok(())
        })();

        std::env::set_current_dir(previous_cwd)?;
        match previous_xdg_config_home {
            Some(path) => std::env::set_var("XDG_CONFIG_HOME", path),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match previous_home {
            Some(path) => std::env::set_var("HOME", path),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&temp_dir);

        result
    }
}
