use env_logger::Env;
use futures_util::TryFutureExt;
use log::info;
use ollana::{discovery::ServerDiscovery, manager::Manager, ollama::Ollama, proxy::ServerProxy};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    let local_ollama = Ollama::default();

    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    info!("Starting Ollana...");

    match detect_mode(local_ollama).await {
        Mode::Server => run_server_mode().await,
        Mode::Client => run_client_mode().await,
    }
}

async fn run_server_mode() -> anyhow::Result<()> {
    let server_proxy = ServerProxy::default();
    let server_discovery = ServerDiscovery::default();

    info!("Running in Server Mode");

    tokio::select! {
        val = tokio::signal::ctrl_c().map_err(anyhow::Error::new) => val,
        val = server_proxy.run_server() => val,
        val = server_discovery.run() => val,
    }
}

async fn run_client_mode() -> anyhow::Result<()> {
    let mut manager = Manager::default();

    info!("Running in Client Mode");

    tokio::select! {
        val = tokio::signal::ctrl_c().map_err(anyhow::Error::new) => val,
        val = manager.run() => val,
    }
}

enum Mode {
    Client,
    Server,
}

async fn detect_mode(ollama: Ollama) -> Mode {
    match ollama.get_version().await {
        Ok(_) => Mode::Server,
        Err(_) => Mode::Client,
    }
}
