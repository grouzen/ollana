use crate::{
    args::ServeArgs, discovery::ServerDiscovery, manager::Manager, ollama::Ollama,
    proxy::ServerProxy,
};
use futures_util::TryFutureExt;
use log::info;

pub struct ServeApp {
    // https://www.man7.org/linux/man-pages/man7/daemon.7.html
    #[allow(dead_code)]
    sysv_daemon: bool,
    local_ollama: Ollama,
}

enum Mode {
    Client,
    Server,
}

impl ServeApp {
    pub fn from_args(args: ServeArgs) -> Self {
        ServeApp {
            sysv_daemon: args.daemon,
            local_ollama: Ollama::default(),
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        info!("Starting Ollana...");

        match self.detect_mode().await {
            Mode::Server => Self::run_server_mode().await,
            Mode::Client => Self::run_client_mode().await,
        }
    }

    async fn detect_mode(&self) -> Mode {
        match self.local_ollama.get_version().await {
            Ok(_) => Mode::Server,
            Err(_) => Mode::Client,
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
}
