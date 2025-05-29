use std::io;

use log::info;
use ollana::{
    discovery::{ClientDiscovery, ServerDiscovery},
    ollama::Ollama,
    proxy::ClientProxy,
    proxy::ServerProxy,
};

#[actix_web::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    info!("Starting Ollana...");
    // To run the proxy we need:
    // 1. check if there is no ollama running on localhost:port where port is a default or configured ollama port
    //    a. in such case, start ServerProxy
    //    b. otherwise, start ClientProxy
    // let client_proxy = ClientProxy::default();
    let ollama = Ollama::default();

    match detect_mode(ollama).await {
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

            let client_proxy = ClientProxy::default();
            let client_discovery = ClientDiscovery::default();

            tokio::select! {
                val = client_proxy.run_server() => val,
                val = client_discovery.run() => val
            }
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
