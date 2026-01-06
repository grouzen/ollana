use std::{collections::HashMap, path::PathBuf, sync::Arc};

use crate::{
    args::ServeArgs,
    certs::Certs,
    client_manager::ClientManager,
    constants,
    device::Device,
    discovery::{convert_providers_to_server_proxy_ports, ServerDiscovery, UdpServerDiscovery},
    proto::ProviderType,
    provider::{LMStudio, LlamaServer, Ollama, Provider, VLLM},
    proxy::{ClientProxy, HttpClientProxy, HttpServerProxy, ServerProxy},
    Mode, PortMapping,
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
    port_mappings: HashMap<ProviderType, PortMapping>,
    allowed_providers: Vec<ProviderType>,
}

impl ServeApp {
    pub fn new(
        args: ServeArgs,
        certs: Arc<dyn Certs>,
        device: Arc<dyn Device>,
    ) -> anyhow::Result<Self> {
        let port_mappings = args
            .get_port_mappings()
            .iter()
            .map(|(k, v)| ((*k).into(), *v))
            .collect();

        // Convert args::ProviderType to proto::ProviderType
        let allowed_providers: Vec<ProviderType> = args
            .get_allowed_providers()
            .into_iter()
            .map(|p| p.into())
            .collect();

        Ok(ServeApp {
            sysv_daemon: args.daemon,
            pid_file: args.pid_file,
            log_file: args.log_file,
            force_server_mode: args.force_server_mode,
            local_ollama: Arc::new(Ollama::default()),
            certs,
            device,
            port_mappings,
            allowed_providers,
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
        // Create providers with custom ports from mappings
        let providers = self.create_providers_with_ports()?;

        // Calculate server proxy ports for each provider
        let proxy_ports = self.calculate_server_proxy_port_mappings(&providers);

        // Create server proxy with custom ports (for now, uses first provider's port)
        // TODO: Support multiple server proxies per provider type
        let server_proxy = if let Some((&provider_type, &proxy_port)) = proxy_ports.iter().next() {
            info!(
                "Starting server proxy for {:?} on port {}",
                provider_type, proxy_port
            );
            HttpServerProxy::builder(self.device.clone())
                .port(proxy_port)
                .build()
        } else {
            HttpServerProxy::new(self.device.clone())
        };

        let server_discovery = UdpServerDiscovery::with_providers_and_ports(
            providers,
            self.allowed_providers.clone(),
            proxy_ports,
        )
        .await?;

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
        let port_mappings = self.port_mappings.clone();
        let allowed_providers = self.allowed_providers.clone();

        let mut manager = ClientManager::new(
            self.device.clone(),
            move |provider_type, server_socket_addr, device| {
                // In client mode:
                // port1 = server proxy port to connect to (from discovery or mapping)
                // port2 = local client proxy port to bind (from mapping or provider default)

                let local_proxy_port = if let Some(mapping) = port_mappings.get(&provider_type) {
                    mapping
                        .port2
                        .unwrap_or_else(|| constants::get_default_client_proxy_port(provider_type))
                } else {
                    constants::get_default_client_proxy_port(provider_type)
                };

                info!(
                    "Creating client proxy for {:?}: connecting to server {}:{}, binding locally to {}",
                    provider_type, server_socket_addr.ip(), server_socket_addr.port(), local_proxy_port
                );

                Ok(Box::new(
                    HttpClientProxy::builder(server_socket_addr, device)
                        .port(local_proxy_port)
                        .build()?,
                ) as Box<dyn ClientProxy>)
            },
            allowed_providers,
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

    /// Create providers with custom LLM ports from port mappings
    fn create_providers_with_ports(
        &self,
    ) -> anyhow::Result<HashMap<ProviderType, Arc<dyn Provider>>> {
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();

        for &provider_type in &self.allowed_providers {
            let provider: Arc<dyn Provider> = match provider_type {
                ProviderType::Ollama => self
                    .port_mappings
                    .get(&provider_type)
                    .and_then(|mapping| mapping.port1)
                    .map(|port1| {
                        info!("Creating Ollama provider with custom LLM port {}", port1);

                        Arc::new(Ollama::default_with_port(port1))
                    })
                    .unwrap_or_else(|| Arc::new(Ollama::default())),
                ProviderType::Vllm => self
                    .port_mappings
                    .get(&provider_type)
                    .and_then(|mapping| mapping.port1)
                    .map(|port1| {
                        info!("Creating vLLM provider with custom LLM port {}", port1);

                        Arc::new(VLLM::default_with_port(port1))
                    })
                    .unwrap_or_else(|| Arc::new(VLLM::default())),
                ProviderType::LmStudio => self
                    .port_mappings
                    .get(&provider_type)
                    .and_then(|mapping| mapping.port1)
                    .map(|port1| {
                        info!("Creating LM Studio provider with custom LLM port {}", port1);
                        Arc::new(LMStudio::default_with_port(port1))
                    })
                    .unwrap_or_else(|| Arc::new(LMStudio::default())),
                ProviderType::LlamaServer => self
                    .port_mappings
                    .get(&provider_type)
                    .and_then(|mapping| mapping.port1)
                    .map(|port1| {
                        info!(
                            "Creating llama.cpp server provider with custom LLM port {}",
                            port1
                        );
                        Arc::new(LlamaServer::default_with_port(port1))
                    })
                    .unwrap_or_else(|| Arc::new(LlamaServer::default())),
                ProviderType::Unspecified => continue,
            };

            providers.insert(provider_type, provider);
        }

        Ok(providers)
    }

    /// Calculate server proxy ports for each provider
    /// In server mode: port2 from mapping, or LLM port + 1 as default
    fn calculate_server_proxy_port_mappings(
        &self,
        providers: &HashMap<ProviderType, Arc<dyn Provider>>,
    ) -> HashMap<ProviderType, u16> {
        convert_providers_to_server_proxy_ports(providers)
            .iter()
            .map(|(provider_type, provider_port)| {
                let mapped_port = match self.port_mappings.get(provider_type) {
                    Some(mapping) => mapping.port2.unwrap_or(*provider_port),
                    None => *provider_port,
                };

                (*provider_type, mapped_port)
            })
            .collect()
    }
}
