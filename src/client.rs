use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::notify::notify_once;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResult {
    pub chat_id: String,
    pub assistant_message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitResult {
    pub status: String, // "completed" | "pending"
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub chat_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub done: bool,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryResult {
    pub chat_id: String,
    pub title: String,
    pub messages: Vec<HistoryEntry>,
}

/// Maximum consecutive transient failures before giving up.
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

pub struct OpenWebUIClient {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenWebUIClient {
    pub fn new(client: Client, base_url: String, api_key: String) -> Self {
        Self {
            client,
            base_url,
            api_key,
        }
    }

    /// Check server connectivity and API key validity.
    pub async fn check_connectivity(&self) -> Result<String> {
        // First check if the server is reachable (no auth)
        let version_url = format!("{}/api/version", self.base_url);
        let resp = self
            .client
            .get(&version_url)
            .send()
            .await
            .context("Failed to connect to Open WebUI server")?;

        if !resp.status().is_success() {
            return Err(anyhow!("Server returned {}", resp.status()));
        }

        let version_body: Value = resp.json().await.unwrap_or_else(|_| json!({}));
        let version = version_body
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Then check API key validity by hitting an auth'd endpoint
        let chat_url = format!("{}/api/v1/chats/list", self.base_url);
        let auth_resp = self
            .client
            .get(&chat_url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("Failed to send auth check request")?;

        match auth_resp.status() {
            s if s.is_success() => Ok(version),
            s if s.as_u16() == 401 => {
                Err(anyhow!("API key is invalid or not recognized (HTTP 401)"))
            }
            s if s.as_u16() == 403 => Err(anyhow!(
                "API key lacks permission for this endpoint (HTTP 403).\n\
                 If endpoint restrictions are enabled, add these paths to\n\
                 auth.api_key.allowed_endpoints in Open WebUI admin settings:\n  {}",
                config_required_endpoints()
            )),
            s => Err(anyhow!("Unexpected status: {}", s)),
        }
    }

    /// List available models.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/api/models", self.base_url);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("Failed to fetch models")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to list models (HTTP {}): {}", status, body));
        }

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse models response")?;

        // /api/models may return {"data": [...]} or a bare array of objects.
        let models: Vec<String> = body
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| body.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        m.get("id")
                            .and_then(|v| v.as_str())
                            .or_else(|| m.get("name").and_then(|v| v.as_str()))
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    /// Delete a persistent chat session.
    pub async fn delete_chat(&self, chat_id: &str) -> Result<bool> {
        let url = format!("{}/api/v1/chats/{}", self.base_url, chat_id);
        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("Failed to delete chat")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !resp.status().is_success() {
            return Err(anyhow!("HTTP {}", resp.status()));
        }

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse delete response")?;
        Ok(body.as_bool().unwrap_or(false))
    }

    /// Fetch all messages from a chat session.
    pub async fn fetch_history(&self, chat_id: &str) -> Result<HistoryResult> {
        let url = format!("{}/api/v1/chats/{}", self.base_url, chat_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("Failed to fetch chat")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("Chat not found: {}", chat_id));
        }
        if !resp.status().is_success() {
            return Err(anyhow!("HTTP {}", resp.status()));
        }

        let body: Value = resp.json().await.context("Failed to parse chat data")?;
        let title = body
            .get("title")
            .and_then(|title| title.as_str())
            .unwrap_or("")
            .to_string();

        let messages = body
            .get("chat")
            .and_then(|chat| chat.get("history"))
            .and_then(|history| history.get("messages"))
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("Could not find messages in chat data"))?;

        let mut entries = Vec::with_capacity(messages.len());
        for message in messages.values() {
            let role = message
                .get("role")
                .and_then(|role| role.as_str())
                .unwrap_or("unknown")
                .to_string();
            let content = extract_content(message);
            let reasoning = extract_reasoning(message);
            let done = message
                .get("done")
                .and_then(|done| done.as_bool())
                .unwrap_or(false);
            let timestamp = message
                .get("timestamp")
                .and_then(|timestamp| timestamp.as_u64())
                .unwrap_or(0);
            entries.push(HistoryEntry {
                role,
                content,
                reasoning,
                done,
                timestamp,
            });
        }

        Ok(HistoryResult {
            chat_id: chat_id.to_string(),
            title,
            messages: entries,
        })
    }

    /// Submit a chat completion request to Open WebUI.
    ///
    /// For a **new chat**: pass `chat_id=None`. The server will create a new
    /// persistent chat and assign a chat_id.
    ///
    /// For a **follow-up**: pass `chat_id=Some(id)`. The message will be
    /// appended to the existing chat.
    pub async fn submit_message(
        &self,
        message: &str,
        model: &str,
        chat_id: Option<&str>,
        _title: Option<&str>,
        web_search: bool,
    ) -> Result<ChatResult> {
        let url = format!("{}/api/chat/completions", self.base_url);

        let user_message_id = Uuid::new_v4().to_string();
        let assistant_message_id = Uuid::new_v4().to_string();
        let session_id = Uuid::new_v4().to_string();

        // Build the request payload for Open WebUI's chat completion endpoint.
        // `parent_id: null` and an omitted `chat_id` identify a new persistent
        // chat. Follow-ups provide both the existing chat ID and a parent ID.
        let parent_id = chat_id.map(|_| Uuid::new_v4().to_string());
        let chat_id_for_req = chat_id.unwrap_or_default().to_string();

        let mut payload = json!({
            "model": model,
            "messages": [
                {
                    "role": "user",
                    "content": message,
                }
            ],
            "stream": true,
            "features": {
                "web_search": web_search,
            },
            "id": assistant_message_id,
            "assistant_message_id": assistant_message_id,
            "session_id": session_id,
            "parent_id": parent_id,
            "metadata": {
                "user_message": {
                    "id": user_message_id,
                    "role": "user",
                    "content": message,
                },
            },
        });

        if let Some(chat_id) = chat_id {
            payload["chat_id"] = json!(chat_id);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .context("Failed to submit chat request")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();

            // Provide helpful error for 403 endpoint restrictions
            if status.as_u16() == 403 {
                return Err(anyhow!(
                    "Open WebUI returned 403 Forbidden.\n\
                     The API key may lack endpoint permissions.\n\
                     Required endpoints:\n  {}\n\
                     Add these to Admin Settings → API Keys → Allowed Endpoints",
                    config_required_endpoints()
                ));
            }

            return Err(anyhow!(
                "Chat submission failed (HTTP {}): {}",
                status,
                body
            ));
        }

        // The async path returns JSON with chat_id
        let body: Value = resp.json().await.context("Failed to parse response")?;

        let resolved_chat_id = body
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(chat_id_for_req);

        Ok(ChatResult {
            chat_id: resolved_chat_id,
            assistant_message_id,
            user_message_id: Some(user_message_id),
        })
    }

    /// Wait for an assistant message to complete by polling the chat data.
    ///
    /// Polls `/api/v1/chats/{id}` at the configured interval, checking the
    /// message's `done` flag and content. Returns when the message is complete
    /// or the timeout expires.
    pub async fn wait_for_completion(
        &self,
        chat_id: &str,
        message_id: &str,
        timeout: u64,
        poll_interval: u64,
        notify_cmd: Option<&str>,
    ) -> Result<WaitResult> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
        let poll_dur = Duration::from_secs(poll_interval);
        let mut consecutive_failures: u32 = 0;

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Ok(WaitResult {
                    status: "pending".to_string(),
                    content: String::new(),
                    reasoning: None,
                    chat_id: chat_id.to_string(),
                    message_id: message_id.to_string(),
                });
            }

            match self.fetch_message(chat_id, message_id).await {
                Ok(Some((content, reasoning, done))) => {
                    consecutive_failures = 0;
                    if done {
                        // Completion is terminal, so this invocation can notify only once.
                        if let Err(e) = notify_once(notify_cmd) {
                            eprintln!("Warning: notification failed: {}", e);
                        }
                        return Ok(WaitResult {
                            status: "completed".to_string(),
                            content,
                            reasoning,
                            chat_id: chat_id.to_string(),
                            message_id: message_id.to_string(),
                        });
                    }
                }
                Ok(None) => {
                    // Message not found yet — chat may still be initializing
                    consecutive_failures = 0;
                }
                Err(e) => {
                    consecutive_failures += 1;
                    eprintln!(
                        "Warning: poll failure ({}/{}): {}",
                        consecutive_failures, MAX_CONSECUTIVE_FAILURES, e
                    );
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        return Err(anyhow!(
                            "Too many consecutive poll failures ({}). Last error: {}",
                            consecutive_failures,
                            e
                        ));
                    }
                }
            }

            tokio::time::sleep(poll_dur).await;
        }
    }

    /// Fetch a specific message from a chat and check if it's done.
    async fn fetch_message(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<Option<(String, Option<String>, bool)>> {
        let url = format!("{}/api/v1/chats/{}", self.base_url, chat_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("Failed to fetch chat")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            return Err(anyhow!("HTTP {}", resp.status()));
        }

        let body: Value = resp.json().await.context("Failed to parse chat data")?;

        // Navigate: body.chat.history.messages[message_id]
        let message = body
            .get("chat")
            .and_then(|c| c.get("history"))
            .and_then(|h| h.get("messages"))
            .and_then(|m| m.get(message_id));

        match message {
            Some(msg) => {
                let content = extract_content(msg);
                let reasoning = extract_reasoning(msg);
                let done = msg.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
                Ok(Some((content, reasoning, done)))
            }
            None => Ok(None),
        }
    }
}

/// Extract regular content text from legacy content or Open WebUI message output.
fn extract_content(message: &Value) -> String {
    let content = message
        .get("content")
        .and_then(|content| content.as_str())
        .unwrap_or("");
    if !content.is_empty() {
        return content.to_string();
    }

    // Fall back to regular output[].content[].text items (OpenWebUI 0.10.x format).
    extract_output_text(message, |item_type| {
        item_type.is_empty() || item_type == "message"
    })
}

/// Extract reasoning/thinking text from Open WebUI reasoning output.
fn extract_reasoning(message: &Value) -> Option<String> {
    let reasoning = extract_output_text(message, |item_type| item_type == "reasoning");
    (!reasoning.is_empty()).then_some(reasoning)
}

fn extract_output_text(message: &Value, include_item_type: impl Fn(&str) -> bool) -> String {
    let Some(output) = message.get("output").and_then(|output| output.as_array()) else {
        return String::new();
    };

    let mut text = String::new();
    for item in output {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if !include_item_type(item_type) {
            continue;
        }
        if let Some(entries) = item.get("content").and_then(Value::as_array) {
            for entry in entries {
                if let Some(entry_text) = entry
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| entry.get("content").and_then(Value::as_str))
                {
                    text.push_str(entry_text);
                }
            }
        }
    }
    text
}

fn config_required_endpoints() -> String {
    crate::config::Config::required_endpoints().join("\n  ")
}

#[cfg(test)]
mod tests {
    use super::OpenWebUIClient;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_once(response: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should have an address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("test server should accept");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            let header_end = loop {
                let read = stream
                    .read(&mut chunk)
                    .await
                    .expect("test server should read request");
                assert_ne!(read, 0, "client closed connection before sending a request");
                request.extend_from_slice(&chunk[..read]);
                if let Some(header_end) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| position + 4)
                {
                    break header_end;
                }
            };
            let headers = std::str::from_utf8(&request[..header_end])
                .expect("request headers should be UTF-8");
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length").then(|| {
                            value
                                .trim()
                                .parse::<usize>()
                                .expect("content length is valid")
                        })
                    })
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let read = stream
                    .read(&mut chunk)
                    .await
                    .expect("test server should read request body");
                assert_ne!(
                    read, 0,
                    "client closed connection before sending the request body"
                );
                request.extend_from_slice(&chunk[..read]);
            }
            stream
                .write_all(response.as_bytes())
                .await
                .expect("test server should write response");
            String::from_utf8(request).expect("request should be UTF-8")
        });

        (format!("http://{address}"), server)
    }

    #[tokio::test]
    async fn delete_chat_sends_authenticated_delete_and_returns_response_boolean() {
        let (base_url, server) = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntrue",
        )
        .await;
        let client = OpenWebUIClient::new(reqwest::Client::new(), base_url, "api-key".to_string());

        assert!(client
            .delete_chat("chat-123")
            .await
            .expect("delete response should succeed"));

        let request = server.await.expect("test server should complete");
        assert!(request.starts_with("DELETE /api/v1/chats/chat-123 HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer api-key\r\n"));
    }

    #[tokio::test]
    async fn delete_chat_returns_false_when_chat_is_not_found() {
        let (base_url, server) =
            serve_once("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        let client = OpenWebUIClient::new(reqwest::Client::new(), base_url, "api-key".to_string());

        assert!(!client
            .delete_chat("missing-chat")
            .await
            .expect("404 should map to false"));
        server.await.expect("test server should complete");
    }

    #[tokio::test]
    async fn submit_message_sends_requested_web_search_feature() {
        let body = r#"{"chat_id":"chat-123"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let (base_url, server) = serve_once(Box::leak(response.into_boxed_str())).await;
        let client = OpenWebUIClient::new(reqwest::Client::new(), base_url, "api-key".to_string());

        let result = client
            .submit_message("Hello", "test-model", None, None, false)
            .await
            .expect("chat submission should succeed");
        assert_eq!(result.chat_id, "chat-123");

        let request = server.await.expect("test server should complete");
        assert!(request.starts_with("POST /api/chat/completions HTTP/1.1\r\n"));
        let (_, request_body) = request
            .split_once("\r\n\r\n")
            .expect("request should include a body");
        let payload: serde_json::Value =
            serde_json::from_str(request_body).expect("request body should be JSON");
        assert_eq!(payload["features"]["web_search"], false);
    }

    #[test]
    fn extracts_content_and_reasoning_from_separate_output_items() {
        let message = serde_json::json!({
            "content": "",
            "output": [
                {"type": "reasoning", "content": [{"text": "Consider "}, {"content": "the prompt."}]},
                {"type": "message", "content": [{"text": "Final "}, {"content": "answer."}]},
                {"content": [{"text": "Untyped output."}]}
            ]
        });

        assert_eq!(
            super::extract_content(&message),
            "Final answer.Untyped output."
        );
        assert_eq!(
            super::extract_reasoning(&message),
            Some("Consider the prompt.".to_string())
        );
    }

    #[test]
    fn extract_content_prefers_legacy_content_and_omits_reasoning() {
        let message = serde_json::json!({
            "content": "Legacy response",
            "output": [{"type": "reasoning", "content": [{"text": "Hidden thought"}]}]
        });

        assert_eq!(super::extract_content(&message), "Legacy response");
        assert_eq!(
            super::extract_reasoning(&message),
            Some("Hidden thought".to_string())
        );
    }

    #[tokio::test]
    async fn fetch_history_returns_all_messages_and_separates_reasoning_from_content() {
        let body = r#"{"title":"History test","chat":{"history":{"messages":{"assistant-direct":{"role":"assistant","content":"Direct content","done":true,"timestamp":100},"assistant-fallback":{"role":"assistant","content":"","done":true,"timestamp":200,"output":[{"type":"reasoning","content":[{"text":"Reasoning"}]},{"type":"message","content":[{"text":"Fallback "},{"content":"content"}]}]}}}}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let (base_url, server) = serve_once(Box::leak(response.into_boxed_str())).await;
        let client = OpenWebUIClient::new(reqwest::Client::new(), base_url, "api-key".to_string());

        let history = client
            .fetch_history("chat-123")
            .await
            .expect("history response should succeed");

        assert_eq!(history.chat_id, "chat-123");
        assert_eq!(history.title, "History test");
        assert_eq!(history.messages.len(), 2);
        assert_eq!(history.messages[0].role, "assistant");
        assert_eq!(history.messages[0].content, "Direct content");
        assert!(history.messages[0].done);
        assert_eq!(history.messages[0].timestamp, 100);
        assert_eq!(history.messages[1].content, "Fallback content");
        assert_eq!(history.messages[1].reasoning, Some("Reasoning".to_string()));
        assert_eq!(history.messages[1].timestamp, 200);

        let request = server.await.expect("test server should complete");
        assert!(request.starts_with("GET /api/v1/chats/chat-123 HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer api-key"));
    }
}
