use serde_json::Value;

/// Browser -> Codex direction
#[derive(Debug, Clone)]
pub enum ToCodexMessage {
    StartTurn {
        prompt: String,
        cwd: Option<String>,
        model: Option<String>,
    },
    Reply {
        thread_id: String,
        prompt: String,
    },
    ApprovalResponse {
        request_id: Value,
        decision: String,
    },
    Interrupt {
        thread_id: String,
        turn_id: String,
    },
}

/// Codex -> Browser direction (broadcast to all connected clients)
#[derive(Debug, Clone)]
pub enum FromCodexMessage {
    Ready,
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    ItemStarted {
        item_id: String,
        item_type: String,
        item: Value,
    },
    Delta {
        item_id: String,
        delta_type: String,
        content: String,
    },
    ItemCompleted {
        item_id: String,
        item_type: String,
        item: Value,
    },
    ApprovalRequest {
        request_id: Value,
        method: String,
        detail: Value,
    },
    TurnCompleted {
        turn_id: String,
        status: String,
        error: Option<Value>,
    },
    Error {
        error: String,
    },
    Disconnected,
}
