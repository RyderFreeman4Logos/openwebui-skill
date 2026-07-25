# openwebui-chat

Use when the agent needs to chat with a model hosted on Open WebUI, creating a persistent, web-visible chat record. This skill provides the workflow for `start` → `wait` → `send` operations via the `openwebui-chat` CLI.

## Prerequisites

- `openwebui-chat` binary installed and on PATH
- Environment configured: `OPENWEBUI_BASE_URL`, `OPENWEBUI_API_KEY`, `OPENWEBUI_DEFAULT_MODEL`
- Optional: `~/.config/openwebui-chat/config.toml` for persistent settings

## Workflow

### 1. Start a new conversation

```bash
result="$(openwebui-chat start \
  --title "Agent to Open WebUI" \
  --message "Your message here.")"
```

Parse the JSON output to get IDs:

```bash
chat_id="$(printf '%s' "$result" | jq -r '.chat_id')"
assistant_id="$(printf '%s' "$result" | jq -r '.assistant_message_id')"
```

`start` returns immediately after the message is submitted — it does **not** wait for the response.

### 2. Wait for the assistant response

```bash
result="$(openwebui-chat wait \
  --chat-id "$chat_id" \
  --message-id "$assistant_id")"

status="$(printf '%s' "$result" | jq -r '.status')"
content="$(printf '%s' "$result" | jq -r '.content')"
```

- `status` is `"completed"` (response done) or `"pending"` (timed out).
- Default timeout is 3300 seconds. Override with `--timeout <seconds>`.
- `wait` blocks until the response completes or the timeout expires. No repeated agent turns needed during polling.

### 3. Send a follow-up message

```bash
result="$(openwebui-chat send \
  --chat-id "$chat_id" \
  --message "Your follow-up message.")"

assistant_id="$(printf '%s' "$result" | jq -r '.assistant_message_id')"

result="$(openwebui-chat wait \
  --chat-id "$chat_id" \
  --message-id "$assistant_id")"
```

The follow-up stays in the **same** chat and is visible at `/c/<chat_id>` in Open WebUI.

## Multi-turn pattern

```bash
# Turn 1
res="$(openwebui-chat start --message "What is 2+2?")"
cid="$(echo "$res" | jq -r '.chat_id')"
mid="$(echo "$res" | jq -r '.assistant_message_id')"
openwebui-chat wait --chat-id "$cid" --message-id "$mid"

# Turn 2 (same chat)
res="$(openwebui-chat send --chat-id "$cid" --message "Now multiply that by 3.")"
mid="$(echo "$res" | jq -r '.assistant_message_id')"
openwebui-chat wait --chat-id "$cid" --message-id "$mid"
```

## Commands reference

| Command | Purpose |
|---------|---------|
| `start` | Create a new chat with first message. Returns chat_id + message_id immediately. |
| `send` | Append a message to an existing chat. |
| `wait` | Block until the assistant response is complete (or timeout). |
| `doctor` | Check connectivity, API key validity, and print required endpoints. |
| `models` | List available models on the server. |

## Notes

- **Persistent records**: every conversation is stored as a native Open WebUI chat, visible at `/c/<chat_id>`.
- **Auto-titles**: Open WebUI automatically generates a title for each chat session.
- **Model selection**: use `--model <name>` to override the default model per invocation.
- **No duplicate transcript**: Open WebUI is the single source of truth — do not store transcripts separately.
- **Diagnostics**: if you get HTTP 403, run `openwebui-chat doctor` — endpoint restrictions may need configuration.
