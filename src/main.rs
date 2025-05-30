use log::info;
use ollana::{discovery::ServerDiscovery, manager::Manager, ollama::Ollama, proxy::ServerProxy};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    info!("Starting Ollana...");
    // To run the proxy we need:
    // 1. check if there is no ollama running on localhost:port where port is a default or configured ollama port
    //    a. in such case, start ServerProxy
    //    b. otherwise, start ClientProxy
    // let client_proxy = ClientProxy::default();
    let local_ollama = Ollama::default();

    match detect_mode(local_ollama).await {
        Mode::Server => {
            info!("Running in Server Mode");

            let server_proxy = ServerProxy::default();
            let server_discovery = ServerDiscovery::default();

            tokio::select! {
                val = server_proxy.run_server() => val,
                val = server_discovery.run() => val,
            }
        }
        Mode::Client => {
            info!("Running in Client Mode");

            Manager::default().run().await
        }
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
