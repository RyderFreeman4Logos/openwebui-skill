mod client;
mod config;
mod notify;

use std::{io::Write, process::ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "openwebui-chat",
    version,
    about = "Chat with Open WebUI models from the command line — creates persistent, web-visible chat records"
)]
struct Cli {
    /// Open WebUI base URL (overrides config/env)
    #[arg(long, global = true)]
    base_url: Option<String>,

    /// API key (overrides config/env)
    #[arg(long, global = true)]
    api_key: Option<String>,

    /// Model to use (overrides config/env default model)
    #[arg(long, global = true)]
    model: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a new chat with the first user message
    Start {
        /// Message text to send
        #[arg(long)]
        message: String,

        /// Chat title hint (Open WebUI will auto-generate a title if omitted)
        #[arg(long)]
        title: Option<String>,
    },
    /// Send a follow-up message to an existing chat
    Send {
        /// Chat ID to append to
        #[arg(long)]
        chat_id: String,

        /// Message text to send
        #[arg(long)]
        message: String,
    },
    /// Wait for an assistant message to complete
    Wait {
        /// Chat ID
        #[arg(long)]
        chat_id: String,

        /// Assistant message ID to wait for
        #[arg(long)]
        message_id: String,

        /// Maximum wait time in seconds
        #[arg(long)]
        timeout: Option<u64>,

        /// Polling interval in seconds
        #[arg(long)]
        poll_interval: Option<u64>,

        /// Optional notification command to run when complete
        #[arg(long)]
        notify: Option<String>,
    },
    /// Delete a chat session
    DeleteSession {
        /// Chat ID to delete
        #[arg(long)]
        chat_id: String,
    },
    /// Check connectivity and diagnose configuration
    Doctor {
        /// Print the required API endpoints for endpoint restrictions
        #[arg(long)]
        print_required_endpoints: bool,
    },
    /// List available models
    Models,
    /// Initialize or update configuration interactively
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Set up configuration interactively
    Init,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {}", e);
            let mut source = e.source();
            while let Some(s) = source {
                eprintln!("  ↳ {}", s);
                source = s.source();
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    // `config init` is the bootstrap path, so it must not require an existing config.
    if matches!(
        &cli.command,
        Command::Config {
            command: ConfigCommand::Init
        }
    ) {
        return run_config_init().await;
    }

    // Handle doctor --print-required-endpoints without needing a config
    if let Command::Doctor {
        print_required_endpoints: true,
    } = cli.command
    {
        println!("Required API endpoints for Open WebUI API key endpoint restrictions:");
        println!();
        println!("Add these paths to Admin Settings → API Keys → Allowed Endpoints:");
        println!("  (or set auth.api_key.allowed_endpoints in the database/config)");
        println!();
        for ep in config::Config::required_endpoints() {
            println!("  {}", ep);
        }
        println!();
        println!("Note: Paths are prefix-matched. '/api/v1/chats' covers sub-paths like '/api/v1/chats/<id>'.");
        return Ok(());
    }

    let cfg = config::Config::load(
        cli.base_url.as_deref(),
        cli.api_key.as_deref(),
        cli.model.as_deref(),
        None,
        None,
    )?;

    let http_client = cfg.http_client()?;
    let client =
        client::OpenWebUIClient::new(http_client, cfg.base_url.clone(), cfg.api_key.clone());

    match cli.command {
        Command::Start { message, title } => {
            let model = resolve_model(&cfg.default_model)?;
            let result = client
                .submit_message(&message, &model, None, title.as_deref())
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Send { chat_id, message } => {
            let model = resolve_model(&cfg.default_model)?;
            let result = client
                .submit_message(&message, &model, Some(&chat_id), None)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Wait {
            chat_id,
            message_id,
            timeout,
            poll_interval,
            notify,
        } => {
            let result = client
                .wait_for_completion(
                    &chat_id,
                    &message_id,
                    timeout.unwrap_or(cfg.timeout),
                    poll_interval.unwrap_or(cfg.poll_interval),
                    notify.as_deref(),
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::DeleteSession { chat_id } => {
            let deleted = client.delete_chat(&chat_id).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "deleted": deleted,
                    "chat_id": chat_id,
                }))?
            );
        }
        Command::Doctor {
            print_required_endpoints: false,
        } => {
            run_doctor(&client, &cfg).await?;
        }
        Command::Doctor {
            print_required_endpoints: true,
        } => unreachable!("handled before configuration is loaded"),
        Command::Models => {
            let models = client.list_models().await?;
            if models.is_empty() {
                println!("No models available.");
            } else {
                println!("Available models:");
                for m in &models {
                    println!("  {}", m);
                }
            }
        }
        Command::Config {
            command: ConfigCommand::Init,
        } => unreachable!("handled before configuration is loaded"),
    }

    Ok(())
}

async fn run_config_init() -> Result<()> {
    let existing = config::Config::load_xdg()?.unwrap_or_default();
    let base_url_default = if existing.base_url.is_empty() {
        "http://localhost:8080".to_string()
    } else {
        existing.base_url.clone()
    };

    println!("openwebui-chat configuration setup");
    println!("===================================");
    println!();

    let base_url = prompt_for_value(
        "Open WebUI base URL",
        &base_url_default,
        Some(base_url_default.clone()),
    )?;
    let api_key = prompt_for_value(
        "API key",
        &existing.api_key,
        (!existing.api_key.is_empty()).then(|| mask_api_key(&existing.api_key)),
    )?;
    let default_model = prompt_for_value(
        "Default model (leave empty to skip)",
        &existing.default_model,
        (!existing.default_model.is_empty()).then(|| existing.default_model.clone()),
    )?;

    let config = config::Config {
        base_url,
        api_key,
        default_model,
        timeout: existing.timeout,
        poll_interval: existing.poll_interval,
    };
    let path = config::Config::config_path()?;
    config.save_to_xdg()?;

    println!();
    println!("Configuration saved to: {}", path.display());
    println!();
    println!("Running diagnostics...");

    let http_client = config.http_client()?;
    let client =
        client::OpenWebUIClient::new(http_client, config.base_url.clone(), config.api_key.clone());
    run_doctor(&client, &config).await
}

fn prompt_for_value(
    label: &str,
    current_value: &str,
    displayed_current_value: Option<String>,
) -> Result<String> {
    match displayed_current_value {
        Some(value) => print!("{} [{}]: ", label, value),
        None => print!("{}: ", label),
    }
    std::io::stdout()
        .flush()
        .context("Failed to flush configuration prompt")?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read configuration input")?;
    let input = input.trim();

    Ok(if input.is_empty() {
        current_value.to_string()
    } else {
        input.to_string()
    })
}

fn mask_api_key(api_key: &str) -> String {
    let characters: Vec<char> = api_key.chars().collect();
    if characters.len() <= 7 {
        return "*".repeat(characters.len());
    }

    let prefix: String = characters[..3].iter().collect();
    let suffix: String = characters[characters.len() - 4..].iter().collect();
    format!("{}...{}", prefix, suffix)
}

fn resolve_model(default_model: &str) -> Result<String> {
    if default_model.is_empty() {
        Err(anyhow::anyhow!(
            "No model specified. Use --model, set OPENWEBUI_DEFAULT_MODEL, \
             or add 'default_model' to config.toml"
        ))
    } else {
        Ok(default_model.to_string())
    }
}

async fn run_doctor(client: &client::OpenWebUIClient, cfg: &config::Config) -> Result<()> {
    println!("openwebui-chat diagnostics");
    println!("==========================");
    println!();
    println!("Configuration:");
    println!("  base_url:       {}", cfg.base_url);
    println!("  api_key:        {}", mask_api_key(&cfg.api_key));
    println!(
        "  default_model:  {}",
        if cfg.default_model.is_empty() {
            "(none)"
        } else {
            &cfg.default_model
        }
    );
    println!("  timeout:        {}s", cfg.timeout);
    println!("  poll_interval:  {}s", cfg.poll_interval);
    println!();

    // Check connectivity
    match client.check_connectivity().await {
        Ok(version) => {
            println!("✓ Server reachable (Open WebUI v{})", version);
        }
        Err(e) => {
            println!("✗ Server connection issue: {}", e);
            println!();
            println!("Required endpoints (if endpoint restrictions are enabled):");
            for ep in config::Config::required_endpoints() {
                println!("  {}", ep);
            }
            return Err(e);
        }
    }

    // Try to list models
    match client.list_models().await {
        Ok(models) if !models.is_empty() => {
            println!("✓ API key valid ({} models available)", models.len());
            if !cfg.default_model.is_empty() {
                if models.contains(&cfg.default_model) {
                    println!("✓ Default model '{}' is available", cfg.default_model);
                } else {
                    println!(
                        "⚠ Default model '{}' not found in available models",
                        cfg.default_model
                    );
                    println!("  Available: {}", models.join(", "));
                }
            }
        }
        Ok(_) => {
            println!("⚠ API key valid but no models available");
        }
        Err(e) => {
            println!("✗ Cannot list models: {}", e);
            println!();
            println!("Required endpoints (if endpoint restrictions are enabled):");
            for ep in config::Config::required_endpoints() {
                println!("  {}", ep);
            }
        }
    }

    println!();
    println!("All checks complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn config_init_is_a_valid_command() {
        assert!(Cli::try_parse_from(["openwebui-chat", "config", "init"]).is_ok());
    }

    #[test]
    fn delete_session_is_a_valid_command() {
        assert!(Cli::try_parse_from(
            ["openwebui-chat", "delete-session", "--chat-id", "chat-123",]
        )
        .is_ok());
    }
}
