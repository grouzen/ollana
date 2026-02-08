use std::{collections::HashMap, sync::Arc};

use log::{debug, error, info};
use tokio::sync::{mpsc, oneshot};
use url::Url;

use crate::{
    certs::Certs,
    constants,
    device::Device,
    proto::ProviderType,
    server_proxy::{HttpServerProxy, ServerProxy},
};

/// Represents an active server proxy for a provider
pub struct ActiveServerProxy {
    proxy: Box<dyn ServerProxy>,
    #[allow(dead_code)] // May be useful for logging/debugging
    provider_port: u16,
}

/// Commands for the server manager event loop
pub enum ServerManagerCommand {
    /// Start a proxy for a provider that became alive
    Start(ProviderType, u16), // (provider_type, provider_port)
    /// Stop a proxy for a provider that went offline
    Stop(ProviderType),
}

/// Manages server proxies for all LLM providers dynamically
/// Starts proxies when providers come online, stops them when they go offline
pub struct ServerManager {
    active_proxies: HashMap<ProviderType, ActiveServerProxy>,
    device: Arc<dyn Device>,
    certs: Arc<dyn Certs>,
    /// Maps provider type to its configured proxy port
    proxy_ports: HashMap<ProviderType, u16>,
}

impl ServerManager {
    pub fn new(
        device: Arc<dyn Device>,
        certs: Arc<dyn Certs>,
        proxy_ports: HashMap<ProviderType, u16>,
    ) -> Self {
        Self {
            active_proxies: HashMap::new(),
            device,
            certs,
            proxy_ports,
        }
    }

    /// Run the server manager, returns the command sender for external use
    pub async fn run(
        &mut self,
        mut cmd_rx: mpsc::Receiver<ServerManagerCommand>,
    ) -> anyhow::Result<()> {
        // TODO: replace with while loop
        loop {
            if let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    ServerManagerCommand::Start(provider_type, provider_port) => {
                        self.handle_start(provider_type, provider_port).await?;
                    }
                    ServerManagerCommand::Stop(provider_type) => {
                        self.handle_stop(provider_type).await?;
                    }
                }
            }
        }
    }

    async fn handle_start(
        &mut self,
        provider_type: ProviderType,
        provider_port: u16,
    ) -> anyhow::Result<()> {
        // Don't start if already running
        if self.active_proxies.contains_key(&provider_type) {
            debug!(
                "Server proxy for {:?} is already running, skipping start",
                provider_type
            );
        } else {
            let proxy_port = self
                .proxy_ports
                .get(&provider_type)
                .copied()
                .unwrap_or_else(|| provider_port + 1);

            let provider_url = Self::build_provider_url(provider_type, provider_port)?;

            info!(
                "Starting server proxy for {:?} on port {}, forwarding to {}",
                provider_type, proxy_port, provider_url
            );

            let mut server_proxy =
                HttpServerProxy::builder(self.device.clone(), self.certs.clone())
                    .port(proxy_port)
                    .provider_url(provider_url)
                    .build();

            let (tx, rx) = oneshot::channel();

            actix_web::rt::spawn(async move {
                if let Err(e) = server_proxy.run_server(tx).await {
                    error!("Server proxy error: {}", e);
                }
            });

            match rx.await {
                Ok(proxy) => {
                    self.active_proxies.insert(
                        provider_type,
                        ActiveServerProxy {
                            proxy,
                            provider_port,
                        },
                    );

                    info!(
                        "Successfully started server proxy for {:?} on port {}",
                        provider_type, proxy_port
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to receive server proxy handle for {:?}: {}",
                        provider_type, e
                    );
                }
            }
        }

        Ok(())
    }

    async fn handle_stop(&mut self, provider_type: ProviderType) -> anyhow::Result<()> {
        if let Some(active_proxy) = self.active_proxies.remove(&provider_type) {
            info!("Stopping server proxy for {:?}", provider_type);

            active_proxy.proxy.stop(true).await;
        } else {
            debug!(
                "No active server proxy for {:?}, skipping stop",
                provider_type
            );
        }

        Ok(())
    }

    fn build_provider_url(provider_type: ProviderType, port: u16) -> anyhow::Result<Url> {
        let address = match provider_type {
            ProviderType::Ollama => constants::OLLAMA_DEFAULT_ADDRESS,
            ProviderType::Vllm => constants::VLLM_DEFAULT_ADDRESS,
            ProviderType::LmStudio => constants::LMSTUDIO_DEFAULT_ADDRESS,
            ProviderType::LlamaServer => constants::LLAMA_SERVER_DEFAULT_ADDRESS,
            ProviderType::Unspecified => constants::OLLAMA_DEFAULT_ADDRESS,
        };

        let url_str = format!("http://{}:{}", address, port);
        Url::parse(&url_str).map_err(anyhow::Error::from)
    }
}
