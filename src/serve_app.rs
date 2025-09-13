use std::path::PathBuf;

use crate::{
    args::ServeArgs, discovery::ServerDiscovery, manager::Manager, ollama::Ollama,
    proxy::ServerProxy,
};
use daemonizr::{Daemonizr, Group, Stderr, Stdout, User};
use futures_util::TryFutureExt;
use log::{error, info};
use tokio::signal::unix::{signal, SignalKind};

pub struct ServeApp {
    // https://www.man7.org/linux/man-pages/man7/daemon.7.html
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

    pub fn run(&self) -> anyhow::Result<()> {
        info!("Starting Ollana...");

        if self.sysv_daemon {
            Self::daemonize()?;
        }

        actix_web::rt::System::new().block_on(self.detect_mode_and_run())
    }

    async fn detect_mode_and_run(&self) -> anyhow::Result<()> {
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

        // Prepare signal futures
        let mut sigterm = signal(SignalKind::terminate())?;

        tokio::select! {
            // Cross-platform ctrl_c support
            _ = tokio::signal::ctrl_c().map_err(anyhow::Error::new) => {
                info!("Received Ctrl-c (SIGINT), shutting down Server Mode...");
                Ok(())
            },
            // Unix: SIGTERM
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down Server Mode...");
                Ok(())
            }
            val = server_proxy.run_server() => val,
            val = server_discovery.run() => val,
        }
    }

    async fn run_client_mode() -> anyhow::Result<()> {
        let mut manager = Manager::default();

        info!("Running in Client Mode");

        // Prepare signal futures
        let mut sigterm = signal(SignalKind::terminate())?;

        tokio::select! {
            // Cross-platform ctrl_c support
            _ = tokio::signal::ctrl_c().map_err(anyhow::Error::new) => {
                info!("Received Ctrl-c (SIGINT), shutting down Client Mode...");
                Ok(())
            },
            // Unix: SIGTERM
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down Client Mode...");
                Ok(())
            }
            val = manager.run() => val,
        }
    }

    fn daemonize() -> anyhow::Result<()> {
        let user = User::by_name("ollana")?;
        let group = Group::by_name("ollana")?;
        let daemonizr = Daemonizr::new().work_dir(PathBuf::from("/var/lib/ollana"))?;
        let daemonizr = daemonizr.umask(0o137)?;
        let daemonizr = daemonizr
            .pidfile(PathBuf::from("/run/ollana/ollana.pid"))
            .as_user(user)
            .as_group(group)
            .stdout(Stdout::Redirect(PathBuf::from("/var/log/ollana/serve.log")))
            .stderr(Stderr::Redirect(PathBuf::from("/var/log/ollana/serve.log")));

        daemonizr
            .spawn()
            .inspect(|_| info!("Running in daemon mode"))
            .inspect_err(|_| error!("Failed to daemonize the application"))
            .map_err(anyhow::Error::new)
    }
}
