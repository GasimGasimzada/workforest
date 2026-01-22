use axum::{extract::State, routing::get, Router};
use workforest_core::config_dir;
use serde::Serialize;
use std::{error::Error, net::SocketAddr, sync::Arc};
use tokio::sync::oneshot;

#[derive(Clone)]
struct AppState {
    shutdown_sender: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
}

#[derive(Serialize)]
struct ServerMetadata {
    pid: u32,
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let state = AppState {
        shutdown_sender: Arc::new(tokio::sync::Mutex::new(Some(shutdown_sender))),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/shutdown", get(shutdown))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = listener.local_addr()?;

    write_metadata(local_addr)?;

    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver))
        .await?;

    remove_metadata();

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn shutdown(State(state): State<AppState>) -> &'static str {
    let mut sender_guard = state.shutdown_sender.lock().await;
    if let Some(sender) = sender_guard.take() {
        let _ = sender.send(());
    }
    "shutting down"
}

async fn wait_for_shutdown(mut shutdown_receiver: oneshot::Receiver<()>) {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = &mut shutdown_receiver => {},
    }
}

fn write_metadata(addr: SocketAddr) -> Result<(), Box<dyn Error>> {
    let config_dir = config_dir();
    std::fs::create_dir_all(&config_dir)?;

    let metadata = ServerMetadata {
        pid: std::process::id(),
        port: addr.port(),
    };

    let metadata_path = config_dir.join("server.json");
    let data = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(metadata_path, data)?;

    Ok(())
}

fn remove_metadata() {
    let metadata_path = config_dir().join("server.json");
    let _ = std::fs::remove_file(metadata_path);
}
