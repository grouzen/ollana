use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use log::{debug, error, info};
use prost::Message;
use tokio::{
    net::UdpSocket,
    sync::{mpsc::Sender, Mutex},
    time,
};
use tokio_stream::wrappers::IntervalStream;

use crate::{
    constants,
    manager::ManagerCommand,
    proto::{DiscoveryRequest, DiscoveryResponse, ProviderInfo, ProviderType},
    provider::{LMStudio, LlamaServer, Ollama as OllamaProvider, Provider, VLLM},
};

const RANDOM_UDP_PORT: u16 = 0;
const DEFAULT_CLIENT_BROADCAST_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_SERVER_LIVENESS_INTERVAL: Duration = Duration::from_secs(10);
const DISCOVERY_BUFFER_SIZE: usize = 1024;

pub const DEFAULT_ALLOWED_PROVIDERS: &[ProviderType] = &[
    ProviderType::Ollama,
    ProviderType::Vllm,
    ProviderType::LmStudio,
    ProviderType::LlamaServer,
];

pub fn create_default_providers() -> HashMap<ProviderType, Arc<dyn Provider>> {
    let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
    providers.insert(ProviderType::Ollama, Arc::new(OllamaProvider::default()));
    providers.insert(ProviderType::Vllm, Arc::new(VLLM::default()));
    providers.insert(ProviderType::LmStudio, Arc::new(LMStudio::default()));
    providers.insert(ProviderType::LlamaServer, Arc::new(LlamaServer::default()));
    providers
}

/// Trait for abstracting network operations in ClientDiscovery.
/// This allows for mocking network behavior in tests.
#[async_trait]
pub trait ClientDiscoveryNetwork: Send + Sync {
    /// Send a discovery request to the broadcast address.
    async fn send(&self, request: &DiscoveryRequest) -> io::Result<usize>;

    /// Receive a discovery response from the network.
    async fn recv(&self) -> io::Result<(DiscoveryResponse, SocketAddr)>;
}

/// Trait for abstracting network operations in ServerDiscovery.
/// This allows for mocking network behavior in tests.
#[async_trait]
pub trait ServerDiscoveryNetwork: Send + Sync {
    /// Receive a discovery request from the network.
    async fn recv(&self) -> io::Result<(DiscoveryRequest, SocketAddr)>;

    /// Send a discovery response to a specific address.
    async fn send(&self, response: &DiscoveryResponse, addr: SocketAddr) -> io::Result<usize>;
}

/// UDP-based implementation of ClientDiscoveryNetwork.
pub struct UdpClientDiscoveryNetwork {
    socket: UdpSocket,
    server_port: u16,
}

impl UdpClientDiscoveryNetwork {
    pub async fn new(server_port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, RANDOM_UDP_PORT)).await?;
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            server_port,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

#[async_trait]
impl ClientDiscoveryNetwork for UdpClientDiscoveryNetwork {
    async fn send(&self, request: &DiscoveryRequest) -> io::Result<usize> {
        let mut buf = Vec::with_capacity(DISCOVERY_BUFFER_SIZE);
        request.encode(&mut buf).map_err(|e| {
            error!("Failed to encode DiscoveryRequest: {}", e);
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?;

        let len = self
            .socket
            .send_to(&buf, (Ipv4Addr::BROADCAST, self.server_port))
            .await
            .inspect_err(|error| error!("Client discovery error while sending: {}", error))?;

        debug!("Client discovery sent {} bytes", len);

        Ok(len)
    }

    async fn recv(&self) -> io::Result<(DiscoveryResponse, SocketAddr)> {
        let mut buf = vec![0u8; DISCOVERY_BUFFER_SIZE];
        let (len, addr) = self
            .socket
            .recv_from(&mut buf)
            .await
            .inspect_err(|error| error!("Client discovery error while receiving: {}", error))?;

        debug!("Client discovery received {} bytes from {}", len, addr);

        let response = DiscoveryResponse::decode(&buf[..len]).map_err(|e| {
            debug!("Client discovery failed to decode message: {}", e);
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?;

        Ok((response, addr))
    }
}

/// UDP-based implementation of ServerDiscoveryNetwork.
pub struct UdpServerDiscoveryNetwork {
    socket: UdpSocket,
}

impl UdpServerDiscoveryNetwork {
    pub async fn new(port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
        Ok(Self { socket })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

#[async_trait]
impl ServerDiscoveryNetwork for UdpServerDiscoveryNetwork {
    async fn recv(&self) -> io::Result<(DiscoveryRequest, SocketAddr)> {
        let mut buf = vec![0u8; DISCOVERY_BUFFER_SIZE];
        let (len, addr) = self
            .socket
            .recv_from(&mut buf)
            .await
            .inspect_err(|error| error!("Server discovery error while receiving: {}", error))?;

        debug!("Server discovery received {} bytes from {}", len, addr);

        let request = DiscoveryRequest::decode(&buf[..len]).map_err(|e| {
            debug!("Server discovery failed to decode message: {}", e);
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?;

        Ok((request, addr))
    }

    async fn send(&self, response: &DiscoveryResponse, addr: SocketAddr) -> io::Result<usize> {
        let mut buf = Vec::with_capacity(DISCOVERY_BUFFER_SIZE);
        response.encode(&mut buf).map_err(|e| {
            error!("Failed to encode DiscoveryResponse: {}", e);
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?;

        let len = self
            .socket
            .send_to(&buf, addr)
            .await
            .inspect_err(|error| error!("Server discovery error while sending: {}", error))?;

        debug!("Server discovery sent {} bytes to {}", len, addr);
        Ok(len)
    }
}

pub struct ClientDiscovery {
    network: Arc<dyn ClientDiscoveryNetwork>,
    broadcast_interval: std::time::Duration,
    allowed_providers: Vec<ProviderType>,
}

pub struct ServerDiscovery {
    network: Arc<dyn ServerDiscoveryNetwork>,
    providers: HashMap<ProviderType, Arc<dyn Provider>>,
    alive_providers: Arc<Mutex<HashMap<ProviderType, u16>>>,
    allowed_providers: Vec<ProviderType>,
    liveness_interval: std::time::Duration,
}

impl ClientDiscovery {
    pub async fn new(
        server_port: u16,
        broadcast_interval: Duration,
        allowed_providers: Vec<ProviderType>,
    ) -> io::Result<Self> {
        let network = UdpClientDiscoveryNetwork::new(server_port).await?;
        
        Ok(Self {
            network: Arc::new(network),
            broadcast_interval,
            allowed_providers,
        })
    }

    pub async fn with_defaults() -> io::Result<Self> {
        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            DEFAULT_CLIENT_BROADCAST_INTERVAL,
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
        )
        .await
    }

    pub fn with_network(
        network: Arc<dyn ClientDiscoveryNetwork>,
        broadcast_interval: Duration,
        allowed_providers: Vec<ProviderType>,
    ) -> Self {
        Self {
            network,
            broadcast_interval,
            allowed_providers,
        }
    }

    pub async fn run(&self, cmd_tx: &Sender<ManagerCommand>) -> anyhow::Result<()> {
        info!("Running client discovery...");

        tokio::select! {
            val = self.broadcast_periodically() => val,
            val = self.handle_messages(cmd_tx) => val,
        }
    }

    async fn broadcast_periodically(&self) -> anyhow::Result<()> {
        let mut stream = IntervalStream::new(time::interval(self.broadcast_interval));

        while stream.next().await.is_some() {
            let request = DiscoveryRequest {
                allowed_providers: self.allowed_providers.iter().map(|p| *p as i32).collect(),
            };
            let _ = self.network.send(&request).await;
        }

        Ok(())
    }

    async fn handle_messages(&self, cmd_tx: &Sender<ManagerCommand>) -> anyhow::Result<()> {
        loop {
            if let Ok((response, addr)) = self.network.recv().await {
                debug!(
                    "Client discovery found a server with {} provider(s) at {}",
                    response.provider_info.len(),
                    addr
                );

                for provider_info in response.provider_info {
                    let provider_port = provider_info.port as u16;
                    let http_addr = (addr.ip(), provider_port)
                        .to_socket_addrs()?
                        .next()
                        .ok_or_else(|| {
                            anyhow::Error::msg(format!(
                                "Invalid server proxy address for provider {:?} on port {}",
                                ProviderType::try_from(provider_info.provider_type),
                                provider_port
                            ))
                        })?;

                    cmd_tx
                        .send(ManagerCommand::Add(http_addr))
                        .await
                        .unwrap_or(());
                }
            }
        }
    }
}

impl ServerDiscovery {
    pub async fn new(
        port: u16,
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
        liveness_interval: Duration,
    ) -> io::Result<Self> {
        let network = UdpServerDiscoveryNetwork::new(port).await?;

        Ok(Self {
            network: Arc::new(network),
            providers,
            alive_providers: Arc::new(Mutex::new(HashMap::new())),
            allowed_providers,
            liveness_interval,
        })
    }

    pub async fn with_defaults() -> io::Result<Self> {
        let providers = create_default_providers();
        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            providers,
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
            DEFAULT_SERVER_LIVENESS_INTERVAL,
        )
        .await
    }

    pub async fn with_providers(
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
    ) -> io::Result<Self> {
        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            providers,
            allowed_providers,
            DEFAULT_SERVER_LIVENESS_INTERVAL,
        )
        .await
    }

    pub fn with_network(
        network: Arc<dyn ServerDiscoveryNetwork>,
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
        liveness_interval: Duration,
    ) -> Self {
        Self {
            network,
            providers,
            alive_providers: Arc::new(Mutex::new(HashMap::new())),
            allowed_providers,
            liveness_interval,
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        info!("Running server discovery...");

        tokio::select! {
            val = self.handle_messages() => val,
            val = self.run_liveness_check() => Ok(val),
        }
    }

    async fn handle_messages(&self) -> anyhow::Result<()> {
        loop {
            if let Ok((request, addr)) = self.network.recv().await {
                debug!(
                    "Server discovery parsed request with {} allowed provider(s)",
                    request.allowed_providers.len()
                );

                // Only respond if we have alive providers that match the request
                let alive_providers = self.alive_providers.lock().await;
                let has_matching_providers = request.allowed_providers.iter().any(|&p| {
                    alive_providers.contains_key(
                        &ProviderType::try_from(p).unwrap_or(ProviderType::Unspecified),
                    )
                });

                if has_matching_providers {
                    // Build response while holding lock
                    let provider_info: Vec<ProviderInfo> = alive_providers
                        .iter()
                        .filter_map(|(provider_type, &port)| {
                            let provider_type_i32 = *provider_type as i32;
                            if request.allowed_providers.contains(&provider_type_i32) {
                                Some(ProviderInfo {
                                    provider_type: provider_type_i32,
                                    port: port as u32,
                                })
                            } else {
                                None
                            }
                        })
                        .collect();

                    let response = DiscoveryResponse { provider_info };
                    drop(alive_providers); // Release lock before async send
                    let _ = self.network.send(&response, addr).await;
                }
            }
        }
    }

    async fn run_liveness_check(&self) {
        let mut stream = IntervalStream::new(time::interval(self.liveness_interval));

        while stream.next().await.is_some() {
            debug!("Executing liveness checks for all allowed providers");

            let mut alive_providers = self.alive_providers.lock().await;

            for provider_type in &self.allowed_providers {
                if let Some(provider) = self.providers.get(provider_type) {
                    match provider.health_check().await {
                        Ok(true) => {
                            let port = provider.get_port();
                            if !alive_providers.contains_key(provider_type) {
                                info!(
                                    "Detected {:?} is running on port {}, will start responding for this provider",
                                    provider_type, port
                                );
                                alive_providers.insert(*provider_type, port);
                            }
                        }
                        Ok(false) | Err(_) => {
                            if let Some(port) = alive_providers.remove(provider_type) {
                                info!(
                                    "Detected {:?} on port {} is no longer running, will stop responding for this provider",
                                    provider_type, port
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
