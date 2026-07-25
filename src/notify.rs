use std::process::Command;

use anyhow::{Context, Result};

/// Send a notification exactly once when the response is complete.
///
/// If a command is provided via --notify, it is executed once.
/// Failures are non-fatal (logged to stderr).
pub fn notify_once(command: Option<&str>) -> Result<()> {
    let cmd = match command {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(()), // No notification configured — silent success
    };

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(());
    }

    let program = parts[0];
    let args = &parts[1..];

    Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to spawn notification command: {}", program))?;

    Ok(())
}
