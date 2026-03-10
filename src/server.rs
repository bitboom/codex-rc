use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use tokio::sync::{broadcast, mpsc};

use crate::relay::channels::{FromCodexMessage, ToCodexMessage};
use crate::relay::ws_handler::handle_ws;

const INDEX_HTML: &str = include_str!("../ui/index.html");

#[derive(Clone)]
pub struct AppState {
    pub to_codex_tx: mpsc::Sender<ToCodexMessage>,
    pub from_codex_tx: broadcast::Sender<FromCodexMessage>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn index_handler() -> impl IntoResponse {
    Html(INDEX_HTML)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let to_codex_tx = state.to_codex_tx.clone();
    let from_codex_rx = state.from_codex_tx.subscribe();

    ws.on_upgrade(move |socket| handle_ws(socket, to_codex_tx, from_codex_rx))
}
