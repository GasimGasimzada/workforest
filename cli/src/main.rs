use clap::{Parser, Subcommand};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::{error::Error, path::PathBuf, process::Command, thread, time::Duration};
use workforest_core::config_dir;

#[derive(Parser)]
#[command(name = "workforest")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    StopServer,
}

#[derive(Deserialize)]
struct ServerMetadata {
    #[allow(dead_code)]
    pid: u32,
    port: u16,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::StopServer) => stop_server(),
        None => run_tui(),
    }
}

fn run_tui() -> Result<(), Box<dyn Error>> {
    let metadata = ensure_server_running()?;
    let server_url = format!("http://127.0.0.1:{}", metadata.port);

    let tui_binary = locate_binary("workforest-tui")?;
    let status = Command::new(tui_binary)
        .env("WORKFOREST_SERVER_URL", server_url)
        .status()?;

    if !status.success() {
        return Err("tui exited with non-zero status".into());
    }

    Ok(())
}

fn stop_server() -> Result<(), Box<dyn Error>> {
    let metadata = match read_metadata()? {
        Some(metadata) => metadata,
        None => {
            println!("server not running");
            return Ok(());
        }
    };

    let url = format!("http://127.0.0.1:{}/shutdown", metadata.port);
    let client = Client::new();

    let response = client.get(url).send();
    match response {
        Ok(_) => {
            wait_for_server_shutdown();
            println!("server stopped");
        }
        Err(_) => {
            remove_metadata();
            println!("server not reachable; metadata cleared");
        }
    }

    Ok(())
}

fn ensure_server_running() -> Result<ServerMetadata, Box<dyn Error>> {
    if let Some(metadata) = read_metadata()? {
        if is_server_alive(metadata.port) {
            return Ok(metadata);
        }
        remove_metadata();
    }

    start_server()?;

    for _ in 0..20 {
        if let Some(metadata) = read_metadata()? {
            if is_server_alive(metadata.port) {
                return Ok(metadata);
            }
        }
        thread::sleep(Duration::from_millis(150));
    }

    Err("server failed to start".into())
}

fn start_server() -> Result<(), Box<dyn Error>> {
    let server_binary = locate_binary("workforest-server")?;
    Command::new(server_binary).spawn()?;
    Ok(())
}

fn is_server_alive(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/health", port);
    Client::new()
        .get(url)
        .send()
        .map(|resp| resp.status().is_success())
        .unwrap_or(false)
}

fn wait_for_server_shutdown() {
    for _ in 0..20 {
        if read_metadata().ok().flatten().is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(150));
    }
}

fn read_metadata() -> Result<Option<ServerMetadata>, Box<dyn Error>> {
    let metadata_path = metadata_path();
    if !metadata_path.exists() {
        return Ok(None);
    }

    let data = std::fs::read_to_string(metadata_path)?;
    let metadata = serde_json::from_str(&data)?;
    Ok(Some(metadata))
}

fn remove_metadata() {
    let metadata_path = metadata_path();
    let _ = std::fs::remove_file(metadata_path);
}

fn metadata_path() -> PathBuf {
    config_dir().join("server.json")
}

fn locate_binary(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            let candidate = parent.join(name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(format!(
        "{} not found. Build with `cargo build --workspace` or ensure it is in PATH.",
        name
    )
    .into())
}
