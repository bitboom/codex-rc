mod codex;
mod relay;
mod server;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info};

use codex::client::CodexClient;
use relay::channels::{FromCodexMessage, ToCodexMessage};
use server::{AppState, create_router};

#[derive(Parser)]
#[command(name = "codex-rc", about = "Remote control Codex via app-server")]
struct Cli {
    /// Port to listen on
    #[arg(short, long, default_value = "9876")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codex_rc=info".into()),
        )
        .init();

    let cli = Cli::parse();

    info!("Spawning codex app-server...");
    let (to_codex_tx, to_codex_rx) = mpsc::channel::<ToCodexMessage>(32);
    let (from_codex_tx, _) = broadcast::channel::<FromCodexMessage>(256);

    let codex_client = CodexClient::spawn(to_codex_rx, from_codex_tx.clone())
        .await
        .context("Failed to start codex app-server")?;

    let state = AppState {
        to_codex_tx,
        from_codex_tx,
    };

    let router = create_router(state);
    let addr = format!("0.0.0.0:{}", cli.port);
    let listener = TcpListener::bind(&addr)
        .await
        .context(format!("Failed to bind to {addr}"))?;

    let url = build_url(cli.port);
    print_banner(&url);

    tokio::select! {
        result = axum::serve(listener, router) => {
            if let Err(e) = result {
                error!("HTTP server error: {e}");
            }
        }
        result = codex_client.run() => {
            if let Err(e) = result {
                error!("Codex client error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down...");
        }
    }

    Ok(())
}

fn build_url(port: u16) -> String {
    let ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "localhost".to_string());
    format!("http://{ip}:{port}")
}

fn print_banner(url: &str) {
    println!();
    println!("  codex-rc v0.2");
    println!("  ────────────────────────────");
    println!("  URL: {url}");
    println!();

    use qrcode::QrCode;
    if let Ok(code) = QrCode::new(url.as_bytes()) {
        let string = code
            .render::<char>()
            .quiet_zone(true)
            .module_dimensions(2, 1)
            .build();
        println!("{string}");
    }

    println!();
    println!("  Scan QR or open URL on your phone");
    println!("  Press Ctrl+C to stop");
    println!();
}
