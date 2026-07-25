use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub timestamp: u64,
}

/// Append a message to the local session file.
pub fn append_message(chat_id: &str, msg: &LocalMessage) -> Result<()> {
    let path = session_path(chat_id)?;
    let parent = path
        .parent()
        .context("Session file path must have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create session directory: {}", parent.display()))?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open session file for append: {}", path.display()))?;
    serde_json::to_writer(&mut file, msg)
        .with_context(|| format!("Failed to serialize session message: {}", path.display()))?;
    file.write_all(b"\n").with_context(|| {
        format!(
            "Failed to append newline to session file: {}",
            path.display()
        )
    })?;

    Ok(())
}

/// Read all messages from a local session file.
/// Returns None if the file doesn't exist.
pub fn read_session(chat_id: &str) -> Result<Option<Vec<LocalMessage>>> {
    let path = session_path(chat_id)?;
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to open session file: {}", path.display()))
        }
    };

    let mut messages = Vec::new();
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| {
            format!(
                "Failed to read session file {} at line {}",
                path.display(),
                line_number + 1
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let message = serde_json::from_str(&line).with_context(|| {
            format!(
                "Failed to parse session file {} at line {}",
                path.display(),
                line_number + 1
            )
        })?;
        messages.push(message);
    }

    Ok(Some(messages))
}

/// Get the session file path for a chat_id.
pub fn session_path(chat_id: &str) -> Result<PathBuf> {
    let data_dir = dirs::data_dir().context("Could not determine the local data directory")?;
    Ok(data_dir
        .join("openwebui-chat")
        .join("sessions")
        .join(format!("{chat_id}.jsonl")))
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        sync::{Mutex, OnceLock},
    };

    use super::{append_message, read_session, session_path, LocalMessage};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[test]
    fn appends_and_reads_jsonl_messages_from_xdg_data_directory() {
        let _lock = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("environment lock should not be poisoned");
        let original_data_home = env::var_os("XDG_DATA_HOME");
        let temp_data_home = env::temp_dir().join(format!(
            "openwebui-chat-session-test-{}",
            uuid::Uuid::new_v4()
        ));
        env::set_var("XDG_DATA_HOME", &temp_data_home);

        let chat_id = "chat-123";
        let user = LocalMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            reasoning: None,
            timestamp: 100,
        };
        let assistant = LocalMessage {
            role: "assistant".to_string(),
            content: "Hi there".to_string(),
            reasoning: Some("I should greet the user.".to_string()),
            timestamp: 101,
        };

        assert_eq!(
            read_session(chat_id).expect("missing session should be readable"),
            None
        );
        append_message(chat_id, &user).expect("user message should append");
        append_message(chat_id, &assistant).expect("assistant message should append");

        assert_eq!(
            session_path(chat_id).expect("session path should resolve"),
            temp_data_home
                .join("openwebui-chat")
                .join("sessions")
                .join("chat-123.jsonl")
        );
        assert_eq!(
            read_session(chat_id)
                .expect("session should be readable")
                .expect("session should exist"),
            vec![user, assistant]
        );

        if let Some(value) = original_data_home {
            env::set_var("XDG_DATA_HOME", value);
        } else {
            env::remove_var("XDG_DATA_HOME");
        }
        fs::remove_dir_all(temp_data_home).expect("test data directory should be removable");
    }
}
