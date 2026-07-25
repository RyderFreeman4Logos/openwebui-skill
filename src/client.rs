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
    pub chat_id: String,
    pub message_id: String,
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
                    chat_id: chat_id.to_string(),
                    message_id: message_id.to_string(),
                });
            }

            match self.fetch_message(chat_id, message_id).await {
                Ok(Some((content, done))) => {
                    consecutive_failures = 0;
                    if done {
                        // Completion is terminal, so this invocation can notify only once.
                        if let Err(e) = notify_once(notify_cmd) {
                            eprintln!("Warning: notification failed: {}", e);
                        }
                        return Ok(WaitResult {
                            status: "completed".to_string(),
                            content,
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
    ) -> Result<Option<(String, bool)>> {
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
                let content = msg
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|content| !content.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        msg.get("output")
                            .and_then(|output| output.as_array())
                            .map(|output| {
                                let mut text = String::new();
                                for item in output {
                                    if let Some(entries) =
                                        item.get("content").and_then(|content| content.as_array())
                                    {
                                        for entry in entries {
                                            if let Some(entry_text) = entry
                                                .get("text")
                                                .and_then(|text| text.as_str())
                                                .or_else(|| {
                                                    entry
                                                        .get("content")
                                                        .and_then(|content| content.as_str())
                                                })
                                            {
                                                text.push_str(entry_text);
                                            }
                                        }
                                    }
                                }
                                text
                            })
                            .unwrap_or_default()
                    });
                let done = msg.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
                Ok(Some((content, done)))
            }
            None => Ok(None),
        }
    }
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
            loop {
                let read = stream
                    .read(&mut chunk)
                    .await
                    .expect("test server should read request");
                assert_ne!(read, 0, "client closed connection before sending a request");
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
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
}
