use std::io;

use log::info;
use ollana::{client_proxy::ClientProxy, ollama::Ollama, server_proxy::ServerProxy};

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
            ServerProxy::default().run_server().await
        }
        Mode::Client => {
            info!("Running in Client Mode");
            ClientProxy::default().run_server().await
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
