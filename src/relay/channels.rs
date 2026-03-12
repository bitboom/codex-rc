use serde_json::Value;

/// Browser -> Codex direction
#[derive(Debug, Clone)]
pub enum ToCodexMessage {
    StartTurn {
        client_id: u64,
        prompt: String,
        cwd: Option<String>,
        model: Option<String>,
    },
    Reply {
        client_id: u64,
        thread_id: String,
        prompt: String,
    },
    ApprovalResponse {
        client_id: u64,
        request_id: Value,
        decision: String,
    },
    Interrupt {
        client_id: u64,
        thread_id: String,
        turn_id: String,
    },
}

/// Codex -> Browser direction.
#[derive(Debug, Clone)]
pub enum FromCodexMessage {
    Ready,
    TurnStarted {
        client_id: u64,
        thread_id: String,
        turn_id: String,
    },
    ItemStarted {
        client_id: u64,
        thread_id: String,
        turn_id: String,
        item_id: String,
        item_type: String,
        item: Value,
    },
    Delta {
        client_id: u64,
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta_type: String,
        content: String,
    },
    ItemCompleted {
        client_id: u64,
        thread_id: String,
        turn_id: String,
        item_id: String,
        item_type: String,
        item: Value,
    },
    ApprovalRequest {
        client_id: u64,
        thread_id: String,
        turn_id: String,
        item_id: String,
        request_id: Value,
        method: String,
        detail: Value,
    },
    TurnCompleted {
        client_id: u64,
        thread_id: String,
        turn_id: String,
        status: String,
        error: Option<Value>,
    },
    Error {
        client_id: Option<u64>,
        error: String,
    },
    Disconnected,
}
