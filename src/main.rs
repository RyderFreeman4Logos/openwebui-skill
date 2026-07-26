mod client;
mod config;
mod notify;
mod session;

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

        /// Disable web search for this message, overriding the config setting
        #[arg(long)]
        no_web_search: bool,
    },
    /// Send a follow-up message to an existing chat
    Send {
        /// Chat ID to append to
        #[arg(long)]
        chat_id: String,

        /// Message text to send
        #[arg(long)]
        message: String,

        /// Disable web search for this message, overriding the config setting
        #[arg(long)]
        no_web_search: bool,
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
    /// Display a chat session's history, preferring local storage
    History {
        /// Chat ID to read
        #[arg(long)]
        chat_id: String,

        /// Page number from the end (1 = last page)
        #[arg(long)]
        page: Option<usize>,

        /// Messages per page
        #[arg(long)]
        page_size: Option<usize>,

        /// Output format: markdown (default) or json
        #[arg(long, value_parser = ["markdown", "json"])]
        format: Option<String>,
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

    if let Command::History {
        chat_id,
        page,
        page_size,
        format,
    } = &cli.command
    {
        if let Some(messages) = session::read_session(chat_id)? {
            let result = local_history_result(chat_id, messages);
            print_history(&result, *page, *page_size, format.as_deref())?;
            return Ok(());
        }
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
        Command::Start {
            message,
            title,
            no_web_search,
        } => {
            let model = resolve_model(&cfg.default_model)?;
            let web_search = resolve_web_search(cfg.web_search, no_web_search);
            let result = client
                .submit_message(&message, &model, None, title.as_deref(), web_search)
                .await?;
            session::append_message(
                &result.chat_id,
                &session::LocalMessage {
                    role: "user".to_string(),
                    content: message.clone(),
                    reasoning: None,
                    timestamp: current_unix_timestamp(),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Send {
            chat_id,
            message,
            no_web_search,
        } => {
            let model = resolve_model(&cfg.default_model)?;
            let web_search = resolve_web_search(cfg.web_search, no_web_search);
            let result = client
                .submit_message(&message, &model, Some(&chat_id), None, web_search)
                .await?;
            session::append_message(
                &result.chat_id,
                &session::LocalMessage {
                    role: "user".to_string(),
                    content: message.clone(),
                    reasoning: None,
                    timestamp: current_unix_timestamp(),
                },
            )?;
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
            if result.status == "completed" {
                session::append_message(
                    &result.chat_id,
                    &session::LocalMessage {
                        role: "assistant".to_string(),
                        content: result.content.clone(),
                        reasoning: result.reasoning.clone(),
                        timestamp: current_unix_timestamp(),
                    },
                )?;
            }
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::History {
            chat_id,
            page,
            page_size,
            format,
        } => {
            let result = client.fetch_history(&chat_id).await?;
            print_history(&result, page, page_size, format.as_deref())?;
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

fn current_unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn local_history_result(
    chat_id: &str,
    messages: Vec<session::LocalMessage>,
) -> client::HistoryResult {
    client::HistoryResult {
        chat_id: chat_id.to_string(),
        title: format!("Chat {chat_id}"),
        messages: messages
            .into_iter()
            .map(|message| client::HistoryEntry {
                role: message.role,
                content: message.content,
                reasoning: message.reasoning,
                done: true,
                timestamp: message.timestamp,
            })
            .collect(),
    }
}

fn print_history(
    result: &client::HistoryResult,
    page: Option<usize>,
    page_size: Option<usize>,
    format: Option<&str>,
) -> Result<()> {
    match format.unwrap_or("markdown") {
        "json" => println!("{}", serde_json::to_string_pretty(result)?),
        "markdown" => print!(
            "{}",
            render_history_markdown(
                &result.title,
                &result.chat_id,
                &result.messages,
                page.unwrap_or(1),
                page_size.unwrap_or(10),
            )?
        ),
        unsupported => return Err(anyhow::anyhow!("Unsupported history format: {unsupported}")),
    }
    Ok(())
}

fn render_history_markdown(
    title: &str,
    chat_id: &str,
    messages: &[client::HistoryEntry],
    page: usize,
    page_size: usize,
) -> Result<String> {
    if page == 0 {
        return Err(anyhow::anyhow!("--page must be at least 1"));
    }
    if page_size == 0 {
        return Err(anyhow::anyhow!("--page-size must be at least 1"));
    }

    let message_count = messages.len();
    let total_pages = message_count / page_size + usize::from(message_count % page_size != 0);
    let mut markdown = format!(
        "# {title}\n\n> Session: {chat_id} | Page {page}/{total_pages} | {message_count} messages total\n\n---\n"
    );

    if page > total_pages {
        markdown.push_str(&format!(
            "\nPage {page} exceeds total pages ({total_pages}).\n"
        ));
        return Ok(markdown);
    }

    let end = message_count - (page - 1) * page_size;
    let start = end.saturating_sub(page_size);
    for (index, message) in messages[start..end].iter().enumerate() {
        let heading = match message.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            role => role,
        };
        markdown.push_str(&format!("\n## {heading}\n{}\n", message.content));
        if let Some(reasoning) = &message.reasoning {
            markdown.push_str(&format!(
                "\n<details>\n<summary>Reasoning</summary>\n\n{reasoning}\n\n</details>\n"
            ));
        }
        if index + 1 < end - start {
            markdown.push_str("\n---\n");
        }
    }

    Ok(markdown)
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
        web_search: existing.web_search,
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

fn resolve_web_search(config_web_search: bool, no_web_search: bool) -> bool {
    config_web_search && !no_web_search
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
    println!("  web_search:     {}", cfg.web_search);
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
    use super::{resolve_web_search, Cli};
    use clap::Parser;

    #[test]
    fn config_init_is_a_valid_command() {
        assert!(Cli::try_parse_from(["openwebui-chat", "config", "init"]).is_ok());
    }

    #[test]
    fn start_and_send_accept_no_web_search_flag() {
        assert!(Cli::try_parse_from([
            "openwebui-chat",
            "start",
            "--message",
            "Hello",
            "--no-web-search",
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "openwebui-chat",
            "send",
            "--chat-id",
            "chat-123",
            "--message",
            "Hello again",
            "--no-web-search",
        ])
        .is_ok());
    }

    #[test]
    fn no_web_search_flag_overrides_configured_web_search_setting() {
        assert!(resolve_web_search(true, false));
        assert!(!resolve_web_search(true, true));
        assert!(!resolve_web_search(false, false));
    }

    #[test]
    fn delete_session_is_a_valid_command() {
        assert!(Cli::try_parse_from(
            ["openwebui-chat", "delete-session", "--chat-id", "chat-123",]
        )
        .is_ok());
    }

    #[test]
    fn history_accepts_pagination_and_format_options() {
        assert!(Cli::try_parse_from([
            "openwebui-chat",
            "history",
            "--chat-id",
            "chat-123",
            "--page",
            "2",
            "--page-size",
            "5",
            "--format",
            "json",
        ])
        .is_ok());
    }

    #[test]
    fn markdown_history_paginates_from_the_end_and_includes_reasoning() {
        let messages = vec![
            crate::client::HistoryEntry {
                role: "user".to_string(),
                content: "first".to_string(),
                reasoning: None,
                done: true,
                timestamp: 1,
            },
            crate::client::HistoryEntry {
                role: "assistant".to_string(),
                content: "second".to_string(),
                reasoning: Some("because".to_string()),
                done: true,
                timestamp: 2,
            },
            crate::client::HistoryEntry {
                role: "user".to_string(),
                content: "third".to_string(),
                reasoning: None,
                done: true,
                timestamp: 3,
            },
        ];

        let markdown = super::render_history_markdown("Thread", "chat-123", &messages, 1, 1)
            .expect("valid pagination should render");

        assert_eq!(
            markdown,
            "# Thread\n\n> Session: chat-123 | Page 1/3 | 3 messages total\n\n---\n\n## User\nthird\n"
        );

        let previous_page = super::render_history_markdown("Thread", "chat-123", &messages, 2, 1)
            .expect("valid pagination should render");
        assert!(previous_page.contains("## Assistant\nsecond"));
        assert!(previous_page.contains("<summary>Reasoning</summary>\n\nbecause"));
        assert!(!previous_page.contains("third"));
    }

    #[test]
    fn markdown_history_reports_pages_beyond_available_messages() {
        let markdown = super::render_history_markdown("Thread", "chat-123", &[], 2, 10)
            .expect("valid pagination should render");

        assert_eq!(
            markdown,
            "# Thread\n\n> Session: chat-123 | Page 2/0 | 0 messages total\n\n---\n\nPage 2 exceeds total pages (0).\n"
        );
    }
}
