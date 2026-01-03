use std::{path::PathBuf, sync::Arc};

use crate::{
    args::ServeArgs,
    certs::Certs,
    client_manager::ClientManager,
    constants,
    device::Device,
    discovery::{
        create_default_providers, ServerDiscovery, UdpServerDiscovery, DEFAULT_ALLOWED_PROVIDERS,
    },
    provider::{Ollama, Provider},
    proxy::{ClientProxy, HttpClientProxy, HttpServerProxy, ServerProxy},
    Mode,
};
use daemonizr::{Daemonizr, Group, Stderr, Stdout, User};
use futures_util::TryFutureExt;
use log::{error, info, warn};
use tokio::signal::unix::{signal, SignalKind};

const DEFAULT_LOG_FILE_PATH: &str = "/var/log/ollana/serve.log";
const DEFAULT_PID_FILE_PATH: &str = "/run/ollana.pid";

pub struct ServeApp {
    // https://www.man7.org/linux/man-pages/man7/daemon.7.html
    sysv_daemon: bool,
    pid_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
    force_server_mode: bool,
    local_ollama: Arc<Ollama>,
    certs: Arc<dyn Certs>,
    device: Arc<dyn Device>,
}

impl ServeApp {
    pub fn new(
        args: ServeArgs,
        certs: Arc<dyn Certs>,
        device: Arc<dyn Device>,
    ) -> anyhow::Result<Self> {
        Ok(ServeApp {
            sysv_daemon: args.daemon,
            pid_file: args.pid_file,
            log_file: args.log_file,
            force_server_mode: args.force_server_mode,
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
        if self.force_server_mode {
            warn!("Force server mode is enabled. Ollama may not be available yet during boot.");
            warn!("Requests may fail until Ollama is fully started. ServerDiscovery will handle this automatically.");

            return Mode::Server;
        }

        match self.local_ollama.health_check().await {
            Ok(_) => Mode::Server,
            Err(_) => Mode::Client,
        }
    }

    async fn run_server_mode(&self) -> anyhow::Result<()> {
        let server_proxy = HttpServerProxy::new(self.device.clone());

        // Initialize providers for all supported provider types
        let providers = create_default_providers();

        // Configure which providers to allow (currently all supported)
        let allowed_providers = DEFAULT_ALLOWED_PROVIDERS.to_vec();

        let server_discovery =
            UdpServerDiscovery::with_providers(providers, allowed_providers).await?;

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
            val = server_proxy.run_server(self.certs.as_ref()) => val,
            val = server_discovery.run() => val,
        }
    }

    async fn run_client_mode(&self) -> anyhow::Result<()> {
        let mut manager = ClientManager::new(
            self.device.clone(),
            |provider_type, server_socket_addr, device| {
                let default_provider_port = constants::get_default_client_proxy_port(provider_type);
                let server_socket_addr = (server_socket_addr.ip(), default_provider_port).into();

                Ok(Box::new(HttpClientProxy::new(server_socket_addr, device)?)
                    as Box<dyn ClientProxy>)
            },
        );

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
