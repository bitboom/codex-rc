use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter, Lines};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use crate::relay::channels::{FromCodexMessage, ToCodexMessage};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const APPROVAL_POLICY_ON_REQUEST: &str = "on-request";

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn thread_start_params(cwd: Option<&str>, model: Option<&str>) -> Value {
    let mut params = json!({
        "approvalPolicy": APPROVAL_POLICY_ON_REQUEST,
    });
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    if let Some(model) = model {
        params["model"] = json!(model);
    }
    params
}

pub struct CodexClient {
    child: Child,
    stdin: BufWriter<tokio::process::ChildStdin>,
    stdout_lines: Lines<BufReader<ChildStdout>>,
    stderr_lines: Lines<BufReader<ChildStderr>>,
    to_codex_rx: mpsc::Receiver<ToCodexMessage>,
    from_codex_tx: broadcast::Sender<FromCodexMessage>,
    /// Maps JSON-RPC request IDs we sent → what we're waiting for
    pending_requests: HashMap<u64, PendingRequest>,
    /// Maps server-sent request IDs (for approvals) → item info for forwarding responses
    pending_approvals: HashMap<String, Value>,
    /// Maps thread ids to the websocket client that owns them
    thread_owners: HashMap<String, u64>,
    /// Maps turn ids to the websocket client that owns them
    turn_owners: HashMap<String, u64>,
}

enum PendingRequest {
    ThreadStart {
        client_id: u64,
        /// Original prompt to start a turn after thread is created
        prompt: String,
        model: Option<String>,
    },
    TurnStart { client_id: u64, thread_id: String },
    TurnInterrupt { client_id: u64 },
}

impl PendingRequest {
    fn client_id(&self) -> u64 {
        match self {
            PendingRequest::ThreadStart { client_id, .. }
            | PendingRequest::TurnStart { client_id, .. }
            | PendingRequest::TurnInterrupt { client_id } => *client_id,
        }
    }
}

async fn write_msg(stdin: &mut BufWriter<tokio::process::ChildStdin>, msg: &Value) -> Result<()> {
    let s = serde_json::to_string(msg)?;
    debug!("-> codex: {s}");
    stdin.write_all(s.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

impl CodexClient {
    pub async fn spawn(
        to_codex_rx: mpsc::Receiver<ToCodexMessage>,
        from_codex_tx: broadcast::Sender<FromCodexMessage>,
    ) -> Result<Self> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn `codex app-server`. Is codex in PATH?")?;

        let child_stdin = child.stdin.take().unwrap();
        let child_stdout = child.stdout.take().unwrap();
        let child_stderr = child.stderr.take().unwrap();

        let mut stdin = BufWriter::new(child_stdin);
        let mut stdout_lines = BufReader::new(child_stdout).lines();
        let stderr_lines = BufReader::new(child_stderr).lines();

        // Handshake: initialize
        let init_id = next_id();
        let init_req = json!({
            "method": "initialize",
            "id": init_id,
            "params": {
                "clientInfo": { "name": "codex-rc", "version": "0.2.0" },
                "capabilities": {}
            }
        });
        write_msg(&mut stdin, &init_req).await?;

        let line = stdout_lines
            .next_line()
            .await?
            .context("App-server closed before init response")?;
        debug!("<- codex: {line}");
        let resp: Value = serde_json::from_str(&line)?;
        if resp.get("error").is_some() {
            bail!("App-server init error: {}", resp["error"]);
        }
        info!(
            "App-server initialized: {}",
            resp.get("result")
                .and_then(|r| r.get("userAgent"))
                .and_then(|u| u.as_str())
                .unwrap_or("unknown")
        );

        // Handshake: initialized notification
        let initialized = json!({ "method": "initialized", "params": {} });
        write_msg(&mut stdin, &initialized).await?;

        Ok(Self {
            child,
            stdin,
            stdout_lines,
            stderr_lines,
            to_codex_rx,
            from_codex_tx,
            pending_requests: HashMap::new(),
            pending_approvals: HashMap::new(),
            thread_owners: HashMap::new(),
            turn_owners: HashMap::new(),
        })
    }

    pub async fn run(mut self) -> Result<()> {
        let _ = self.from_codex_tx.send(FromCodexMessage::Ready);

        loop {
            tokio::select! {
                msg = self.to_codex_rx.recv() => {
                    match msg {
                        Some(m) => {
                            if let Err(e) = self.handle_to_codex(m).await {
                                error!("Error handling browser message: {e}");
                            }
                        }
                        None => {
                            info!("All senders dropped, shutting down");
                            break;
                        }
                    }
                }
                line = self.stdout_lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            debug!("<- codex: {line}");
                            if let Err(e) = self.handle_codex_message(&line).await {
                                warn!("Error handling codex message: {e}");
                            }
                        }
                        Ok(None) => {
                            warn!("App-server stdout closed");
                            let _ = self.from_codex_tx.send(FromCodexMessage::Disconnected);
                            break;
                        }
                        Err(e) => {
                            error!("Error reading app-server stdout: {e}");
                            break;
                        }
                    }
                }
                line = self.stderr_lines.next_line() => {
                    match line {
                        Ok(Some(line)) if !line.trim().is_empty() => {
                            debug!("codex stderr: {line}");
                        }
                        Ok(None) | Err(_) => {}
                        _ => {}
                    }
                }
                status = self.child.wait() => {
                    warn!("App-server exited: {status:?}");
                    let _ = self.from_codex_tx.send(FromCodexMessage::Disconnected);
                    break;
                }
            }
        }

        Ok(())
    }

    fn lookup_client_id(&self, thread_id: &str, turn_id: &str) -> Option<u64> {
        self.turn_owners
            .get(turn_id)
            .copied()
            .or_else(|| self.thread_owners.get(thread_id).copied())
    }

    fn register_turn_owner(&mut self, thread_id: &str, turn_id: &str, client_id: u64) {
        if !thread_id.is_empty() {
            self.thread_owners
                .insert(thread_id.to_string(), client_id);
        }
        if !turn_id.is_empty() {
            self.turn_owners.insert(turn_id.to_string(), client_id);
        }
    }

    async fn handle_to_codex(&mut self, msg: ToCodexMessage) -> Result<()> {
        match msg {
            ToCodexMessage::StartTurn {
                client_id,
                prompt,
                cwd,
                model,
            } => {
                // Create a new thread first, then start a turn
                let id = next_id();
                let params = thread_start_params(cwd.as_deref(), model.as_deref());
                let req = json!({
                    "method": "thread/start",
                    "id": id,
                    "params": params,
                });
                self.pending_requests
                    .insert(id, PendingRequest::ThreadStart {
                        client_id,
                        prompt,
                        model,
                    });
                write_msg(&mut self.stdin, &req).await?;
            }
            ToCodexMessage::Reply {
                client_id,
                thread_id,
                prompt,
            } => {
                let id = next_id();
                let req = json!({
                    "method": "turn/start",
                    "id": id,
                    "params": {
                        "threadId": thread_id,
                        "input": [{ "type": "text", "text": prompt }],
                    }
                });
                self.thread_owners.insert(thread_id.clone(), client_id);
                self.pending_requests.insert(
                    id,
                    PendingRequest::TurnStart {
                        client_id,
                        thread_id,
                    },
                );
                write_msg(&mut self.stdin, &req).await?;
            }
            ToCodexMessage::ApprovalResponse {
                client_id: _client_id,
                request_id,
                decision,
            } => {
                // Respond to the server's approval request
                let resp = json!({
                    "id": request_id,
                    "result": decision,
                });
                self.pending_approvals.remove(&request_id.to_string());
                write_msg(&mut self.stdin, &resp).await?;
            }
            ToCodexMessage::Interrupt {
                client_id,
                thread_id,
                turn_id,
            } => {
                let id = next_id();
                let req = json!({
                    "method": "turn/interrupt",
                    "id": id,
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                    }
                });
                self.pending_requests
                    .insert(id, PendingRequest::TurnInterrupt { client_id });
                write_msg(&mut self.stdin, &req).await?;
            }
        }
        Ok(())
    }

    async fn handle_codex_message(&mut self, line: &str) -> Result<()> {
        let val: Value = serde_json::from_str(line)?;

        let has_method = val.get("method").is_some();
        let has_id = val.get("id").is_some();

        if has_method && has_id {
            // Server-to-client request (e.g., approval requests)
            self.handle_server_request(&val);
        } else if has_method {
            // Notification from server
            self.handle_notification(&val);
        } else if has_id {
            // Response to our request
            self.handle_response(&val).await?;
        }

        Ok(())
    }

    fn handle_server_request(&mut self, val: &Value) {
        let method = val["method"].as_str().unwrap_or_default();
        let params = val.get("params").cloned().unwrap_or(json!({}));
        let request_id = val.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping approval request without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };

                self.pending_approvals
                    .insert(request_id.to_string(), request_id.clone());
                let _ = self.from_codex_tx.send(FromCodexMessage::ApprovalRequest {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: params
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    request_id,
                    method: method.to_string(),
                    detail: params,
                });
            }
            "tool/requestUserInput" => {
                // Auto-decline user input requests for now
                warn!("Ignoring tool/requestUserInput");
            }
            _ => {
                debug!("Unhandled server request: {method}");
            }
        }
    }

    fn handle_notification(&mut self, val: &Value) {
        let method = val["method"].as_str().unwrap_or_default();
        let params = val.get("params").cloned().unwrap_or(json!({}));

        match method {
            "turn/started" => {
                let turn = &params["turn"];
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = turn
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping turn/started without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                self.register_turn_owner(thread_id, turn_id, client_id);
                let _ = self.from_codex_tx.send(FromCodexMessage::TurnStarted {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                });
            }
            "turn/completed" => {
                let turn = &params["turn"];
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = turn
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping turn/completed without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let status = turn
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("completed")
                    .to_string();
                let error = turn.get("error").cloned();
                let _ = self.from_codex_tx.send(FromCodexMessage::TurnCompleted {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    status,
                    error,
                });
            }
            "item/started" => {
                let item = &params["item"];
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping item/started without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let _ = self.from_codex_tx.send(FromCodexMessage::ItemStarted {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    item_type: item
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    item: item.clone(),
                });
            }
            "item/completed" => {
                let item = &params["item"];
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping item/completed without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let _ = self.from_codex_tx.send(FromCodexMessage::ItemCompleted {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    item_type: item
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    item: item.clone(),
                });
            }
            "item/agentMessage/delta" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping agentMessage delta without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let _ = self.from_codex_tx.send(FromCodexMessage::Delta {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: params
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    delta_type: "agentMessage".to_string(),
                    content: params
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping reasoning delta without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let _ = self.from_codex_tx.send(FromCodexMessage::Delta {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: params
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    delta_type: "reasoning".to_string(),
                    content: params
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            "item/commandExecution/outputDelta" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping commandExecution delta without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let _ = self.from_codex_tx.send(FromCodexMessage::Delta {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: params
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    delta_type: "commandOutput".to_string(),
                    content: params
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            "item/fileChange/outputDelta" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping fileChange delta without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let _ = self.from_codex_tx.send(FromCodexMessage::Delta {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: params
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    delta_type: "fileChange".to_string(),
                    content: params
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            "item/plan/delta" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let turn_id = params
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(client_id) = self.lookup_client_id(thread_id, turn_id) else {
                    warn!(
                        "Dropping plan delta without known client owner: thread_id={thread_id}, turn_id={turn_id}"
                    );
                    return;
                };
                let _ = self.from_codex_tx.send(FromCodexMessage::Delta {
                    client_id,
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: params
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    delta_type: "plan".to_string(),
                    content: params
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            "thread/status/changed"
            | "thread/tokenUsage/updated"
            | "turn/diff/updated"
            | "turn/plan/updated"
            | "item/reasoning/summaryPartAdded"
            | "thread/started"
            | "serverRequest/resolved" => {
                // Known but not directly relayed to browser
                debug!("Notification (ignored): {method}");
            }
            _ => {
                debug!("Unhandled notification: {method}");
            }
        }
    }

    async fn handle_response(&mut self, val: &Value) -> Result<()> {
        let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or_default();

        let pending = match self.pending_requests.remove(&id) {
            Some(p) => p,
            None => {
                debug!("Response for unknown request id={id}");
                return Ok(());
            }
        };

        if let Some(err) = val.get("error") {
            let error_msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string();
            let _ = self
                .from_codex_tx
                .send(FromCodexMessage::Error {
                    client_id: Some(pending.client_id()),
                    error: error_msg,
                });
            return Ok(());
        }

        let result = val.get("result").cloned().unwrap_or(json!({}));

        match pending {
            PendingRequest::ThreadStart {
                client_id,
                prompt,
                model,
            } => {
                let thread_id = result
                    .get("thread")
                    .and_then(|t| t.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                if thread_id.is_empty() {
                    let _ = self.from_codex_tx.send(FromCodexMessage::Error {
                        client_id: Some(client_id),
                        error: "thread/start returned no thread id".to_string(),
                    });
                    return Ok(());
                }

                info!("Thread created: {thread_id}");
                self.thread_owners.insert(thread_id.clone(), client_id);

                // Now start a turn on this thread
                let request_id = next_id();
                let mut turn_params = json!({
                    "threadId": thread_id,
                    "input": [{ "type": "text", "text": prompt }],
                });
                if let Some(ref m) = model {
                    turn_params["model"] = json!(m);
                }
                let req = json!({
                    "method": "turn/start",
                    "id": request_id,
                    "params": turn_params,
                });
                self.pending_requests.insert(
                    request_id,
                    PendingRequest::TurnStart {
                        client_id,
                        thread_id,
                    },
                );
                write_msg(&mut self.stdin, &req).await?;
            }
            PendingRequest::TurnStart {
                client_id,
                thread_id,
            } => {
                // turn/start response — the streaming will come via notifications
                let turn = &result["turn"];
                if let Some(turn_id) = turn.get("id").and_then(|v| v.as_str()) {
                    self.register_turn_owner(&thread_id, turn_id, client_id);
                }
                debug!(
                    "Turn started: id={}, status={}",
                    turn.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
                    turn.get("status").and_then(|v| v.as_str()).unwrap_or("?"),
                );
            }
            PendingRequest::TurnInterrupt { .. } => {
                debug!("Turn interrupt acknowledged");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::thread_start_params;

    #[test]
    fn thread_start_params_use_current_app_server_approval_policy() {
        let params = thread_start_params(None, None);

        assert_eq!(params["approvalPolicy"], "on-request");
    }

    #[test]
    fn thread_start_params_include_optional_overrides() {
        let params = thread_start_params(Some("/tmp/workspace"), Some("gpt-5"));

        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["cwd"], "/tmp/workspace");
        assert_eq!(params["model"], "gpt-5");
    }
}
