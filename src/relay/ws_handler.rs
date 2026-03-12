use axum::extract::ws::{Message, WebSocket};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use super::channels::{FromCodexMessage, ToCodexMessage};

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

pub async fn handle_ws(
    mut socket: WebSocket,
    to_codex_tx: mpsc::Sender<ToCodexMessage>,
    mut from_codex_rx: broadcast::Receiver<FromCodexMessage>,
) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);

    // Send ready immediately if codex is already up
    let ready = json!({ "type": "ready" });
    if socket
        .send(Message::Text(ready.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    info!("WebSocket client connected");

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!("WS <- browser: {text}");
                        if let Err(e) = handle_browser_message(client_id, &text, &to_codex_tx).await {
                            warn!("Invalid browser message: {e}");
                            let err = json!({"type": "error", "error": e.to_string()});
                            let _ = socket.send(Message::Text(err.to_string().into())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("WebSocket client disconnected");
                        break;
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
            msg = from_codex_rx.recv() => {
                match msg {
                    Ok(codex_msg) => {
                        if !should_forward_to_client(&codex_msg, client_id) {
                            continue;
                        }
                        let json = from_codex_to_json(&codex_msg);
                        debug!("WS -> browser: {json}");
                        if socket.send(Message::Text(json.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WebSocket client lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        let _ = socket.send(Message::Text(
                            json!({"type": "disconnected"}).to_string().into()
                        )).await;
                        break;
                    }
                }
            }
        }
    }
}

async fn handle_browser_message(
    client_id: u64,
    text: &str,
    to_codex_tx: &mpsc::Sender<ToCodexMessage>,
) -> anyhow::Result<()> {
    let val: Value = serde_json::from_str(text)?;
    let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or_default();

    match msg_type {
        "start_turn" => {
            let prompt = val
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let cwd = val
                .get("cwd")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let model = val
                .get("model")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);

            to_codex_tx
                .send(ToCodexMessage::StartTurn {
                    client_id,
                    prompt,
                    cwd,
                    model,
                })
                .await?;
            Ok(())
        }
        "reply" => {
            let thread_id = val
                .get("thread_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let prompt = val
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            to_codex_tx
                .send(ToCodexMessage::Reply {
                    client_id,
                    thread_id,
                    prompt,
                })
                .await?;
            Ok(())
        }
        "approval_response" => {
            let request_id = val.get("request_id").cloned().unwrap_or(Value::Null);
            let decision = val
                .get("decision")
                .and_then(|v| v.as_str())
                .unwrap_or("decline")
                .to_string();

            to_codex_tx
                .send(ToCodexMessage::ApprovalResponse {
                    client_id,
                    request_id,
                    decision,
                })
                .await?;
            Ok(())
        }
        "interrupt" => {
            let thread_id = val
                .get("thread_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let turn_id = val
                .get("turn_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            to_codex_tx
                .send(ToCodexMessage::Interrupt {
                    client_id,
                    thread_id,
                    turn_id,
                })
                .await?;
            Ok(())
        }
        _ => anyhow::bail!("Unknown message type: {msg_type}"),
    }
}

fn should_forward_to_client(msg: &FromCodexMessage, client_id: u64) -> bool {
    match msg {
        FromCodexMessage::Ready | FromCodexMessage::Disconnected => true,
        FromCodexMessage::TurnStarted {
            client_id: owner_id,
            ..
        }
        | FromCodexMessage::ItemStarted {
            client_id: owner_id,
            ..
        }
        | FromCodexMessage::Delta {
            client_id: owner_id,
            ..
        }
        | FromCodexMessage::ItemCompleted {
            client_id: owner_id,
            ..
        }
        | FromCodexMessage::ApprovalRequest {
            client_id: owner_id,
            ..
        }
        | FromCodexMessage::TurnCompleted {
            client_id: owner_id,
            ..
        } => *owner_id == client_id,
        FromCodexMessage::Error {
            client_id: Some(owner_id),
            ..
        } => *owner_id == client_id,
        FromCodexMessage::Error { client_id: None, .. } => true,
    }
}

fn from_codex_to_json(msg: &FromCodexMessage) -> Value {
    match msg {
        FromCodexMessage::Ready => json!({ "type": "ready" }),
        FromCodexMessage::TurnStarted {
            client_id: _,
            thread_id,
            turn_id,
        } => json!({
            "type": "turn_started",
            "thread_id": thread_id,
            "turn_id": turn_id,
        }),
        FromCodexMessage::ItemStarted {
            client_id: _,
            thread_id,
            turn_id,
            item_id,
            item_type,
            item,
        } => json!({
            "type": "item_started",
            "thread_id": thread_id,
            "turn_id": turn_id,
            "item_id": item_id,
            "item_type": item_type,
            "item": item,
        }),
        FromCodexMessage::Delta {
            client_id: _,
            thread_id,
            turn_id,
            item_id,
            delta_type,
            content,
        } => json!({
            "type": "delta",
            "thread_id": thread_id,
            "turn_id": turn_id,
            "item_id": item_id,
            "delta_type": delta_type,
            "content": content,
        }),
        FromCodexMessage::ItemCompleted {
            client_id: _,
            thread_id,
            turn_id,
            item_id,
            item_type,
            item,
        } => json!({
            "type": "item_completed",
            "thread_id": thread_id,
            "turn_id": turn_id,
            "item_id": item_id,
            "item_type": item_type,
            "item": item,
        }),
        FromCodexMessage::ApprovalRequest {
            client_id: _,
            thread_id,
            turn_id,
            item_id,
            request_id,
            method,
            detail,
        } => json!({
            "type": "approval_request",
            "thread_id": thread_id,
            "turn_id": turn_id,
            "item_id": item_id,
            "request_id": request_id,
            "method": method,
            "detail": detail,
        }),
        FromCodexMessage::TurnCompleted {
            client_id: _,
            thread_id,
            turn_id,
            status,
            error,
        } => json!({
            "type": "turn_completed",
            "thread_id": thread_id,
            "turn_id": turn_id,
            "status": status,
            "error": error,
        }),
        FromCodexMessage::Error {
            client_id: _,
            error,
        } => json!({
            "type": "error",
            "error": error,
        }),
        FromCodexMessage::Disconnected => json!({ "type": "disconnected" }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{from_codex_to_json, should_forward_to_client};
    use crate::relay::channels::FromCodexMessage;

    #[test]
    fn item_events_include_turn_and_thread_ids() {
        let message = FromCodexMessage::ItemStarted {
            client_id: 7,
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            item_id: "item-1".to_string(),
            item_type: "agentMessage".to_string(),
            item: json!({"id": "item-1", "type": "agentMessage"}),
        };

        let payload = from_codex_to_json(&message);

        assert_eq!(payload["thread_id"], "thread-1");
        assert_eq!(payload["turn_id"], "turn-1");
        assert_eq!(payload["item_id"], "item-1");
    }

    #[test]
    fn approval_requests_include_routing_fields() {
        let message = FromCodexMessage::ApprovalRequest {
            client_id: 9,
            thread_id: "thread-9".to_string(),
            turn_id: "turn-9".to_string(),
            item_id: "item-9".to_string(),
            request_id: json!("req-9"),
            method: "item/commandExecution/requestApproval".to_string(),
            detail: json!({"command": "ls"}),
        };

        let payload = from_codex_to_json(&message);

        assert_eq!(payload["thread_id"], "thread-9");
        assert_eq!(payload["turn_id"], "turn-9");
        assert_eq!(payload["item_id"], "item-9");
        assert_eq!(payload["request_id"], "req-9");
    }

    #[test]
    fn relayed_turn_events_only_reach_the_owning_client() {
        let message = FromCodexMessage::TurnStarted {
            client_id: 42,
            thread_id: "thread-42".to_string(),
            turn_id: "turn-42".to_string(),
        };

        assert!(should_forward_to_client(&message, 42));
        assert!(!should_forward_to_client(&message, 7));
    }

    #[test]
    fn broadcast_events_still_reach_every_client() {
        assert!(should_forward_to_client(&FromCodexMessage::Ready, 1));
        assert!(should_forward_to_client(&FromCodexMessage::Ready, 2));
        assert!(should_forward_to_client(
            &FromCodexMessage::Error {
                client_id: None,
                error: "global".to_string(),
            },
            1
        ));
    }
}
