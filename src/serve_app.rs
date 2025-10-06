use std::{path::PathBuf, sync::Arc};

use crate::{
    args::ServeArgs, certs::Certs, device::Device, discovery::ServerDiscovery, manager::Manager,
    ollama::Ollama, proxy::ServerProxy, Mode,
};
use daemonizr::{Daemonizr, Group, Stderr, Stdout, User};
use futures_util::TryFutureExt;
use log::{error, info};
use tokio::signal::unix::{signal, SignalKind};

const DEFAULT_LOG_FILE_PATH: &str = "/var/log/ollana/serve.log";
const DEFAULT_PID_FILE_PATH: &str = "/run/ollana.pid";

pub struct ServeApp {
    // https://www.man7.org/linux/man-pages/man7/daemon.7.html
    sysv_daemon: bool,
    pid_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
    local_ollama: Arc<Ollama>,
    certs: Arc<Certs>,
    device: Arc<Device>,
}

impl ServeApp {
    pub fn from_args(
        args: ServeArgs,
        certs: Arc<Certs>,
        device: Arc<Device>,
    ) -> anyhow::Result<Self> {
        Ok(ServeApp {
            sysv_daemon: args.daemon,
            pid_file: args.pid_file,
            log_file: args.log_file,
            local_ollama: Arc::new(Ollama::default()),
            certs,
            device,
        })
    }

    pub fn run(&self) -> anyhow::Result<()> {
        info!("Starting Ollana...");

        if self.sysv_daemon {
            self.daemonize()?;
        }

        actix_web::rt::System::new().block_on(self.detect_mode_and_run())
    }

    async fn detect_mode_and_run(&self) -> anyhow::Result<()> {
        match self.detect_mode().await {
            Mode::Server => self.run_server_mode().await,
            Mode::Client => self.run_client_mode().await,
        }
    }

    async fn detect_mode(&self) -> Mode {
        match self.local_ollama.get_version().await {
            Ok(_) => Mode::Server,
            Err(_) => Mode::Client,
        }
    }

    async fn run_server_mode(&self) -> anyhow::Result<()> {
        let server_proxy = ServerProxy::new(self.device.clone());
        let server_discovery = ServerDiscovery::new(self.local_ollama.clone());

        info!("Running in Server Mode");

        self.certs.gen_http_server()?;

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
            val = server_proxy.run_server(&self.certs) => val,
            val = server_discovery.run() => val,
        }
    }

    async fn run_client_mode(&self) -> anyhow::Result<()> {
        let mut manager = Manager::new(self.device.clone());

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

    fn daemonize(&self) -> anyhow::Result<()> {
        let user = User::by_name("ollana")?;
        let group = Group::by_name("ollana")?;
        let daemonizr = Daemonizr::new().work_dir(PathBuf::from("/var/lib/ollana"))?;
        let daemonizr = daemonizr.umask(0o137)?;
        let pid_path = self
            .pid_file
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PID_FILE_PATH));
        let log_file = self
            .log_file
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_LOG_FILE_PATH));

        let daemonizr = daemonizr
            .pidfile(pid_path.to_path_buf())
            .as_user(user)
            .as_group(group)
            .stdout(Stdout::Redirect(log_file.clone()))
            .stderr(Stderr::Redirect(log_file));

        daemonizr
            .spawn()
            .inspect(|_| info!("Running in daemon mode"))
            .inspect_err(|_| error!("Failed to daemonize the application"))
            .map_err(anyhow::Error::new)
    }
}
