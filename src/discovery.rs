use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

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

pub struct ClientDiscovery {
    server_port: u16,
    broadcast_interval: std::time::Duration,
    allowed_providers: Vec<ProviderType>,
}

pub struct ServerDiscovery {
    port: u16,
    providers: HashMap<ProviderType, Arc<dyn Provider>>,
    alive_providers: Arc<Mutex<HashMap<ProviderType, u16>>>,
    allowed_providers: Vec<ProviderType>,
    liveness_interval: std::time::Duration,
}

impl Default for ClientDiscovery {
    fn default() -> Self {
        Self {
            server_port: constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            broadcast_interval: DEFAULT_CLIENT_BROADCAST_INTERVAL,
            allowed_providers: DEFAULT_ALLOWED_PROVIDERS.to_vec(),
        }
    }
}

impl Default for ServerDiscovery {
    fn default() -> Self {
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(ProviderType::Ollama, Arc::new(OllamaProvider::default()));
        providers.insert(ProviderType::Vllm, Arc::new(VLLM::default()));
        providers.insert(ProviderType::LmStudio, Arc::new(LMStudio::default()));
        providers.insert(ProviderType::LlamaServer, Arc::new(LlamaServer::default()));

        Self {
            port: constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            providers,
            alive_providers: Arc::new(Mutex::new(HashMap::new())),
            allowed_providers: DEFAULT_ALLOWED_PROVIDERS.to_vec(),
            liveness_interval: DEFAULT_SERVER_LIVENESS_INTERVAL,
        }
    }
}

impl ClientDiscovery {
    pub async fn run(&self, cmd_tx: &Sender<ManagerCommand>) -> anyhow::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, RANDOM_UDP_PORT)).await?;
        let local_addr = socket.local_addr()?;
        socket.set_broadcast(true)?;

        info!("Running client discovery on {}...", local_addr);

        tokio::select! {
            val = self.broadcast_periodically(&socket) => val,
            val = self.handle_messages(&socket, cmd_tx) => val,
        }
    }

    async fn broadcast_periodically(&self, socket: &UdpSocket) -> anyhow::Result<()> {
        let mut stream = IntervalStream::new(time::interval(self.broadcast_interval));

        while stream.next().await.is_some() {
            let _ = self.send(socket).await;
        }

        Ok(())
    }

    async fn handle_messages(
        &self,
        socket: &UdpSocket,
        cmd_tx: &Sender<ManagerCommand>,
    ) -> anyhow::Result<()> {
        loop {
            if let Ok((response, addr)) = self.recv(socket).await {
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

    async fn send(&self, socket: &UdpSocket) -> io::Result<usize> {
        let request = DiscoveryRequest {
            allowed_providers: self.allowed_providers.iter().map(|p| *p as i32).collect(),
        };

        let mut buf = Vec::with_capacity(DISCOVERY_BUFFER_SIZE);
        request.encode(&mut buf).map_err(|e| {
            error!("Failed to encode DiscoveryRequest: {}", e);
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?;

        let len = socket
            .send_to(&buf, (Ipv4Addr::BROADCAST, self.server_port))
            .await
            .inspect_err(|error| error!("Client discovery error while sending: {}", error))?;

        debug!("Client discovery sent {} bytes", len);

        Ok(len)
    }

    async fn recv(&self, socket: &UdpSocket) -> io::Result<(DiscoveryResponse, SocketAddr)> {
        let mut buf = vec![0u8; DISCOVERY_BUFFER_SIZE];
        let (len, addr) = socket
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

impl ServerDiscovery {
    pub fn new(
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
    ) -> Self {
        Self {
            providers,
            allowed_providers,
            ..Default::default()
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, self.port)).await?;
        let local_addr = socket.local_addr()?;

        info!("Running server discovery on {}...", local_addr);

        tokio::select! {
            val = self.handle_messages(&socket) => val,
            val = self.run_liveness_check() => Ok(val),
        }
    }

    async fn handle_messages(&self, socket: &UdpSocket) -> anyhow::Result<()> {
        loop {
            if let Ok((request, addr)) = self.recv(socket).await {
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
                    drop(alive_providers); // Release lock before async send
                    let _ = self.send(socket, addr, &request.allowed_providers).await;
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

    async fn recv(&self, socket: &UdpSocket) -> io::Result<(DiscoveryRequest, SocketAddr)> {
        let mut buf = vec![0u8; DISCOVERY_BUFFER_SIZE];
        let (len, addr) = socket
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

    async fn send(
        &self,
        socket: &UdpSocket,
        addr: SocketAddr,
        allowed_providers: &[i32],
    ) -> io::Result<usize> {
        let alive_providers = self.alive_providers.lock().await;

        // Filter alive providers based on client's allowed_providers list
        let provider_info: Vec<ProviderInfo> = alive_providers
            .iter()
            .filter_map(|(provider_type, &port)| {
                let provider_type_i32 = *provider_type as i32;
                if allowed_providers.contains(&provider_type_i32) {
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

        let mut buf = Vec::with_capacity(DISCOVERY_BUFFER_SIZE);
        response.encode(&mut buf).map_err(|e| {
            error!("Failed to encode DiscoveryResponse: {}", e);
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?;

        let len = socket
            .send_to(&buf, addr)
            .await
            .inspect_err(|error| error!("Server discovery error while sending: {}", error))?;

        debug!("Server discovery sent {} bytes to {}", len, addr);
        Ok(len)
    }
}
