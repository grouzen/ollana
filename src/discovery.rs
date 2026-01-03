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
    client_manager::ClientManagerCommand,
    constants,
    proto::{DiscoveryRequest, DiscoveryResponse, ProviderInfo, ProviderType},
    provider::{LMStudio, LlamaServer, Ollama, Provider, VLLM},
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

fn create_default_providers() -> HashMap<ProviderType, Arc<dyn Provider>> {
    let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
    providers.insert(ProviderType::Ollama, Arc::new(Ollama::default()));
    providers.insert(ProviderType::Vllm, Arc::new(VLLM::default()));
    providers.insert(ProviderType::LmStudio, Arc::new(LMStudio::default()));
    providers.insert(ProviderType::LlamaServer, Arc::new(LlamaServer::default()));
    providers
}

pub fn convert_providers_to_server_proxy_ports(
    providers: &HashMap<ProviderType, Arc<dyn Provider>>,
) -> HashMap<ProviderType, u16> {
    let mut proxy_ports = HashMap::new();
    for (provider_type, provider) in providers {
        proxy_ports.insert(*provider_type, provider.get_port() + 1);
    }
    proxy_ports
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

/// Trait for client-side discovery operations.
/// This allows for different implementations of client discovery behavior.
#[async_trait]
pub trait ClientDiscovery: Send + Sync {
    /// Run the client discovery process.
    async fn run(&self, cmd_tx: &Sender<ClientManagerCommand>) -> anyhow::Result<()>;
}

/// Trait for server-side discovery operations.
/// This allows for different implementations of server discovery behavior.
#[async_trait]
pub trait ServerDiscovery: Send + Sync {
    /// Run the server discovery process.
    async fn run(&self) -> anyhow::Result<()>;
}

/// UDP-based implementation of ClientDiscovery.
pub struct UdpClientDiscovery {
    network: Arc<dyn ClientDiscoveryNetwork>,
    broadcast_interval: std::time::Duration,
    allowed_providers: Vec<ProviderType>,
}

/// UDP-based implementation of ServerDiscovery.
pub struct UdpServerDiscovery {
    network: Arc<dyn ServerDiscoveryNetwork>,
    providers: HashMap<ProviderType, Arc<dyn Provider>>,
    alive_providers: Arc<Mutex<HashMap<ProviderType, u16>>>,
    proxy_ports: HashMap<ProviderType, u16>,
    allowed_providers: Vec<ProviderType>,
    liveness_interval: std::time::Duration,
}

impl UdpClientDiscovery {
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

    /// Periodically broadcast discovery requests to find servers.
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

    /// Handle incoming discovery responses from servers.
    async fn handle_messages(&self, cmd_tx: &Sender<ClientManagerCommand>) -> anyhow::Result<()> {
        loop {
            if let Ok((response, server_socket_addr)) = self.network.recv().await {
                debug!(
                    "Client discovery found a server with {} provider(s) at {}",
                    response.provider_info.len(),
                    server_socket_addr
                );

                for provider_info in response.provider_info {
                    let provider_type = ProviderType::try_from(provider_info.provider_type)
                        .map_err(|_| {
                            anyhow::Error::msg(format!(
                                "Unknown provider type: {}",
                                provider_info.provider_type
                            ))
                        })?;
                    let provider_port = provider_info.port as u16;
                    let provider_socket_addr = (server_socket_addr.ip(), provider_port)
                        .to_socket_addrs()?
                        .next()
                        .ok_or_else(|| {
                            anyhow::Error::msg(format!(
                                "Invalid server proxy address for provider {:?} on port {}",
                                provider_type, provider_port
                            ))
                        })?;

                    cmd_tx
                        .send(ClientManagerCommand::Add(
                            provider_type,
                            provider_socket_addr,
                        ))
                        .await
                        .unwrap_or(());
                }
            }
        }
    }
}

#[async_trait]
impl ClientDiscovery for UdpClientDiscovery {
    async fn run(&self, cmd_tx: &Sender<ClientManagerCommand>) -> anyhow::Result<()> {
        info!("Running client discovery...");

        tokio::select! {
            val = self.broadcast_periodically() => val,
            val = self.handle_messages(cmd_tx) => val,
        }
    }
}

impl UdpServerDiscovery {
    pub async fn new(
        port: u16,
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
        proxy_ports: HashMap<ProviderType, u16>,
        liveness_interval: Duration,
    ) -> io::Result<Self> {
        let network = UdpServerDiscoveryNetwork::new(port).await?;

        Ok(Self {
            network: Arc::new(network),
            providers,
            alive_providers: Arc::new(Mutex::new(HashMap::new())),
            proxy_ports,
            allowed_providers,
            liveness_interval,
        })
    }

    pub async fn with_defaults() -> io::Result<Self> {
        let providers = create_default_providers();
        let proxy_ports = convert_providers_to_server_proxy_ports(&providers);

        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            providers,
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
            proxy_ports,
            DEFAULT_SERVER_LIVENESS_INTERVAL,
        )
        .await
    }

    pub async fn with_providers(
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
    ) -> io::Result<Self> {
        let proxy_ports = convert_providers_to_server_proxy_ports(&providers);

        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            providers,
            allowed_providers,
            proxy_ports,
            DEFAULT_SERVER_LIVENESS_INTERVAL,
        )
        .await
    }

    /// Create UdpServerDiscovery with custom providers and proxy ports
    pub async fn with_providers_and_ports(
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
        proxy_ports: HashMap<ProviderType, u16>,
    ) -> io::Result<Self> {
        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            providers,
            allowed_providers,
            proxy_ports,
            DEFAULT_SERVER_LIVENESS_INTERVAL,
        )
        .await
    }

    pub fn with_network(
        network: Arc<dyn ServerDiscoveryNetwork>,
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
        proxy_ports: HashMap<ProviderType, u16>,
        liveness_interval: Duration,
    ) -> Self {
        Self {
            network,
            providers,
            alive_providers: Arc::new(Mutex::new(HashMap::new())),
            proxy_ports,
            allowed_providers,
            liveness_interval,
        }
    }

    /// Handle incoming discovery requests from clients.
    async fn handle_messages(&self) -> anyhow::Result<()> {
        loop {
            if let Ok((request, client_socket_addr)) = self.network.recv().await {
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
                    // Build response with proxy ports (not LLM ports)
                    let provider_info: Vec<ProviderInfo> = alive_providers
                        .iter()
                        .filter_map(|(provider_type, &provider_port)| {
                            let provider_type_i32 = *provider_type as i32;
                            if request.allowed_providers.contains(&provider_type_i32) {
                                // Use proxy port from mappings, not the LLM port
                                let proxy_port = self
                                    .proxy_ports
                                    .get(provider_type)
                                    .copied()
                                    .unwrap_or(provider_port + 1); // Fallback to LLM port + 1

                                Some(ProviderInfo {
                                    provider_type: provider_type_i32,
                                    port: proxy_port as u32,
                                })
                            } else {
                                None
                            }
                        })
                        .collect();

                    let response = DiscoveryResponse { provider_info };
                    drop(alive_providers); // Release lock before async send
                    let _ = self.network.send(&response, client_socket_addr).await;
                }
            }
        }
    }

    /// Periodically check the liveness of providers.
    async fn run_liveness_check(&self) {
        let mut stream = IntervalStream::new(time::interval(self.liveness_interval));

        while stream.next().await.is_some() {
            debug!("Executing liveness checks for all allowed providers");

            let mut alive_providers = self.alive_providers.lock().await;

            for provider_type in &self.allowed_providers {
                if let Some(provider) = self.providers.get(provider_type) {
                    match provider.health_check().await {
                        Ok(true) => {
                            let provider_port = provider.get_port();

                            if !alive_providers.contains_key(provider_type) {
                                info!(
                                    "Detected {:?} is running on port {}, will start responding for this provider",
                                    provider_type, provider_port
                                );
                                alive_providers.insert(*provider_type, provider_port);
                            }
                        }
                        Ok(false) | Err(_) => {
                            if let Some(provider_port) = alive_providers.remove(provider_type) {
                                info!(
                                    "Detected {:?} on port {} is no longer running, will stop responding for this provider",
                                    provider_type, provider_port
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl ServerDiscovery for UdpServerDiscovery {
    async fn run(&self) -> anyhow::Result<()> {
        info!("Running server discovery...");

        tokio::select! {
            val = self.handle_messages() => val,
            val = self.run_liveness_check() => Ok(val),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::sync::mpsc;

    /// Mock implementation of Provider for testing.
    pub struct MockProvider {
        port: u16,
        health_results: Arc<Mutex<Vec<bool>>>,
        default_result: bool,
    }

    impl MockProvider {
        pub fn new(port: u16, health_results: Vec<bool>) -> Self {
            // Reverse so we can pop from the end
            let mut results = health_results.clone();
            results.reverse();
            let default_result = health_results.last().copied().unwrap_or(false);
            Self {
                port,
                health_results: Arc::new(Mutex::new(results)),
                default_result,
            }
        }

        pub fn always_healthy(port: u16) -> Self {
            Self {
                port,
                health_results: Arc::new(Mutex::new(vec![])),
                default_result: true,
            }
        }

        pub fn always_unhealthy(port: u16) -> Self {
            Self {
                port,
                health_results: Arc::new(Mutex::new(vec![])),
                default_result: false,
            }
        }

        pub fn with_error(port: u16) -> MockProviderWithError {
            MockProviderWithError { port }
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn health_check(&self) -> anyhow::Result<bool> {
            let mut results = self.health_results.lock().await;
            if let Some(result) = results.pop() {
                Ok(result)
            } else {
                Ok(self.default_result)
            }
        }

        fn get_port(&self) -> u16 {
            self.port
        }
    }

    /// Mock provider that always returns an error on health check
    pub struct MockProviderWithError {
        port: u16,
    }

    #[async_trait]
    impl Provider for MockProviderWithError {
        async fn health_check(&self) -> anyhow::Result<bool> {
            Err(anyhow::anyhow!("connection refused"))
        }

        fn get_port(&self) -> u16 {
            self.port
        }
    }

    /// Mock implementation of ClientDiscoveryNetwork for testing.
    pub struct MockClientDiscoveryNetwork {
        responses: Arc<Mutex<Vec<(DiscoveryResponse, SocketAddr)>>>,
    }

    impl MockClientDiscoveryNetwork {
        pub fn new(responses: Vec<(DiscoveryResponse, SocketAddr)>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses)),
            }
        }
    }

    #[async_trait]
    impl ClientDiscoveryNetwork for MockClientDiscoveryNetwork {
        async fn send(&self, _request: &DiscoveryRequest) -> io::Result<usize> {
            Ok(0)
        }

        async fn recv(&self) -> io::Result<(DiscoveryResponse, SocketAddr)> {
            let mut responses = self.responses.lock().await;
            if let Some(response) = responses.pop() {
                Ok(response)
            } else {
                // Return an error to break out of the loop when no more responses
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "no more responses",
                ))
            }
        }
    }

    #[tokio::test]
    async fn test_client_run_single_provider() {
        let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 5000);
        let provider_port = 11434u32;

        let response = DiscoveryResponse {
            provider_info: vec![ProviderInfo {
                provider_type: ProviderType::Ollama as i32,
                port: provider_port,
            }],
        };

        let mock_network = Arc::new(MockClientDiscoveryNetwork::new(vec![(
            response,
            server_addr,
        )]));

        let client = UdpClientDiscovery::with_network(
            mock_network,
            DEFAULT_CLIENT_BROADCAST_INTERVAL,
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
        );

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<ClientManagerCommand>(32);

        // Run client discovery in a separate task with a timeout
        let handle = tokio::spawn(async move {
            let _ = client.run(&cmd_tx).await;
        });

        // Wait for the command to be received
        let cmd = tokio::time::timeout(Duration::from_millis(100), cmd_rx.recv())
            .await
            .expect("timeout waiting for command")
            .expect("channel closed");

        match cmd {
            ClientManagerCommand::Add(provider_type, addr) => {
                assert_eq!(provider_type, ProviderType::Ollama);
                assert_eq!(addr.ip(), server_addr.ip());
                assert_eq!(addr.port(), provider_port as u16);
            }
            _ => panic!("expected ClientManagerCommand::Add"),
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_client_run_multiple_providers() {
        let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 5000);

        let response = DiscoveryResponse {
            provider_info: vec![
                ProviderInfo {
                    provider_type: ProviderType::Ollama as i32,
                    port: 11434,
                },
                ProviderInfo {
                    provider_type: ProviderType::Vllm as i32,
                    port: 8000,
                },
            ],
        };

        let mock_network = Arc::new(MockClientDiscoveryNetwork::new(vec![(
            response,
            server_addr,
        )]));

        let client = UdpClientDiscovery::with_network(
            mock_network,
            DEFAULT_CLIENT_BROADCAST_INTERVAL,
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
        );

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<ClientManagerCommand>(32);

        let handle = tokio::spawn(async move {
            let _ = client.run(&cmd_tx).await;
        });

        // Collect both commands
        let mut received_providers = Vec::new();
        for _ in 0..2 {
            let cmd = tokio::time::timeout(Duration::from_millis(100), cmd_rx.recv())
                .await
                .expect("timeout waiting for command")
                .expect("channel closed");

            match cmd {
                ClientManagerCommand::Add(provider_type, addr) => {
                    assert_eq!(addr.ip(), server_addr.ip());
                    received_providers.push((provider_type, addr.port()));
                }
                _ => panic!("expected ClientManagerCommand::Add"),
            }
        }

        received_providers.sort_by_key(|&(_, port)| port);
        assert_eq!(
            received_providers,
            vec![(ProviderType::Vllm, 8000), (ProviderType::Ollama, 11434)]
        );

        handle.abort();
    }

    #[tokio::test]
    async fn test_client_run_from_multiple_servers() {
        let server1_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 5000);
        let server2_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 200)), 5000);

        let response1 = DiscoveryResponse {
            provider_info: vec![ProviderInfo {
                provider_type: ProviderType::Ollama as i32,
                port: 11434,
            }],
        };

        let response2 = DiscoveryResponse {
            provider_info: vec![ProviderInfo {
                provider_type: ProviderType::Vllm as i32,
                port: 8000,
            }],
        };

        // Note: responses are popped from the end, so order is reversed
        let mock_network = Arc::new(MockClientDiscoveryNetwork::new(vec![
            (response2, server2_addr),
            (response1, server1_addr),
        ]));

        let client = UdpClientDiscovery::with_network(
            mock_network,
            DEFAULT_CLIENT_BROADCAST_INTERVAL,
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
        );

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<ClientManagerCommand>(32);

        let handle = tokio::spawn(async move {
            let _ = client.run(&cmd_tx).await;
        });

        // First command should be from server1
        let cmd1 = tokio::time::timeout(Duration::from_millis(100), cmd_rx.recv())
            .await
            .expect("timeout waiting for command")
            .expect("channel closed");

        match cmd1 {
            ClientManagerCommand::Add(provider_type, addr) => {
                assert_eq!(provider_type, ProviderType::Ollama);
                assert_eq!(addr.ip(), server1_addr.ip());
                assert_eq!(addr.port(), 11434);
            }
            _ => panic!("expected ClientManagerCommand::Add"),
        }

        // Second command should be from server2
        let cmd2 = tokio::time::timeout(Duration::from_millis(100), cmd_rx.recv())
            .await
            .expect("timeout waiting for command")
            .expect("channel closed");

        match cmd2 {
            ClientManagerCommand::Add(provider_type, addr) => {
                assert_eq!(provider_type, ProviderType::Vllm);
                assert_eq!(addr.ip(), server2_addr.ip());
                assert_eq!(addr.port(), 8000);
            }
            _ => panic!("expected ClientManagerCommand::Add"),
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_client_run_empty_provider_list() {
        let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 5000);

        let response = DiscoveryResponse {
            provider_info: vec![],
        };

        let mock_network = Arc::new(MockClientDiscoveryNetwork::new(vec![(
            response,
            server_addr,
        )]));

        let client = UdpClientDiscovery::with_network(
            mock_network,
            DEFAULT_CLIENT_BROADCAST_INTERVAL,
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
        );

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<ClientManagerCommand>(32);

        let handle = tokio::spawn(async move {
            let _ = client.run(&cmd_tx).await;
        });

        // Should not receive any commands when provider_info is empty
        let result = tokio::time::timeout(Duration::from_millis(50), cmd_rx.recv()).await;
        assert!(result.is_err(), "should timeout with no commands received");

        handle.abort();
    }

    /// Mock implementation of ServerDiscoveryNetwork for testing.
    pub struct MockServerDiscoveryNetwork {
        requests: Arc<Mutex<Vec<(DiscoveryRequest, SocketAddr)>>>,
        sent_responses: Arc<Mutex<Vec<(DiscoveryResponse, SocketAddr)>>>,
    }

    impl MockServerDiscoveryNetwork {
        pub fn new(requests: Vec<(DiscoveryRequest, SocketAddr)>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(requests)),
                sent_responses: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub async fn get_sent_responses(&self) -> Vec<(DiscoveryResponse, SocketAddr)> {
            self.sent_responses.lock().await.clone()
        }
    }

    #[async_trait]
    impl ServerDiscoveryNetwork for MockServerDiscoveryNetwork {
        async fn recv(&self) -> io::Result<(DiscoveryRequest, SocketAddr)> {
            let mut requests = self.requests.lock().await;
            if let Some(request) = requests.pop() {
                Ok(request)
            } else {
                // Return an error to break out of the loop when no more requests
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "no more requests",
                ))
            }
        }

        async fn send(&self, response: &DiscoveryResponse, addr: SocketAddr) -> io::Result<usize> {
            let mut sent_responses = self.sent_responses.lock().await;
            sent_responses.push((response.clone(), addr));
            Ok(0)
        }
    }

    /// Helper to create a UdpServerDiscovery with pre-populated alive providers
    async fn create_server_discovery_with_alive_providers(
        mock_network: Arc<MockServerDiscoveryNetwork>,
        alive_providers: HashMap<ProviderType, u16>,
    ) -> UdpServerDiscovery {
        let proxy_ports = alive_providers.clone();

        let server = UdpServerDiscovery::with_network(
            mock_network,
            HashMap::new(), // providers not needed for handle_messages tests
            DEFAULT_ALLOWED_PROVIDERS.to_vec(),
            proxy_ports,
            DEFAULT_SERVER_LIVENESS_INTERVAL,
        );
        // Set alive providers directly
        *server.alive_providers.lock().await = alive_providers;
        server
    }

    #[tokio::test]
    async fn test_server_run_responds_with_matching_provider() {
        let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 12345);

        let request = DiscoveryRequest {
            allowed_providers: vec![ProviderType::Ollama as i32],
        };

        // Requests are popped, so only one request here
        let mock_network = Arc::new(MockServerDiscoveryNetwork::new(vec![(
            request,
            client_addr,
        )]));

        let mut alive_providers = HashMap::new();
        alive_providers.insert(ProviderType::Ollama, 11434);

        let server =
            create_server_discovery_with_alive_providers(mock_network.clone(), alive_providers)
                .await;

        // Run server discovery with a timeout - it will exit when mock returns error
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        let sent_responses = mock_network.get_sent_responses().await;
        assert_eq!(sent_responses.len(), 1);

        let (response, addr) = &sent_responses[0];
        assert_eq!(*addr, client_addr);
        assert_eq!(response.provider_info.len(), 1);
        assert_eq!(
            response.provider_info[0].provider_type,
            ProviderType::Ollama as i32
        );
        assert_eq!(response.provider_info[0].port, 11434);
    }

    #[tokio::test]
    async fn test_server_run_responds_with_multiple_matching_providers() {
        let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 12345);

        let request = DiscoveryRequest {
            allowed_providers: vec![ProviderType::Ollama as i32, ProviderType::Vllm as i32],
        };

        let mock_network = Arc::new(MockServerDiscoveryNetwork::new(vec![(
            request,
            client_addr,
        )]));

        let mut alive_providers = HashMap::new();
        alive_providers.insert(ProviderType::Ollama, 11434);
        alive_providers.insert(ProviderType::Vllm, 8000);

        let server =
            create_server_discovery_with_alive_providers(mock_network.clone(), alive_providers)
                .await;

        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        let sent_responses = mock_network.get_sent_responses().await;
        assert_eq!(sent_responses.len(), 1);

        let (response, addr) = &sent_responses[0];
        assert_eq!(*addr, client_addr);
        assert_eq!(response.provider_info.len(), 2);

        let mut ports: Vec<u32> = response.provider_info.iter().map(|p| p.port).collect();
        ports.sort();
        assert_eq!(ports, vec![8000, 11434]);
    }

    #[tokio::test]
    async fn test_server_run_filters_to_requested_providers_only() {
        let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 12345);

        // Client only requests Ollama
        let request = DiscoveryRequest {
            allowed_providers: vec![ProviderType::Ollama as i32],
        };

        let mock_network = Arc::new(MockServerDiscoveryNetwork::new(vec![(
            request,
            client_addr,
        )]));

        // Server has both Ollama and VLLM alive
        let mut alive_providers = HashMap::new();
        alive_providers.insert(ProviderType::Ollama, 11434);
        alive_providers.insert(ProviderType::Vllm, 8000);

        let server =
            create_server_discovery_with_alive_providers(mock_network.clone(), alive_providers)
                .await;

        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        let sent_responses = mock_network.get_sent_responses().await;
        assert_eq!(sent_responses.len(), 1);

        let (response, _) = &sent_responses[0];
        // Should only include Ollama, not VLLM
        assert_eq!(response.provider_info.len(), 1);
        assert_eq!(
            response.provider_info[0].provider_type,
            ProviderType::Ollama as i32
        );
    }

    #[tokio::test]
    async fn test_server_run_no_response_when_no_matching_providers() {
        let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 12345);

        // Client requests VLLM
        let request = DiscoveryRequest {
            allowed_providers: vec![ProviderType::Vllm as i32],
        };

        let mock_network = Arc::new(MockServerDiscoveryNetwork::new(vec![(
            request,
            client_addr,
        )]));

        // Server only has Ollama alive
        let mut alive_providers = HashMap::new();
        alive_providers.insert(ProviderType::Ollama, 11434);

        let server =
            create_server_discovery_with_alive_providers(mock_network.clone(), alive_providers)
                .await;

        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        let sent_responses = mock_network.get_sent_responses().await;
        // Should not send any response since there's no matching provider
        assert_eq!(sent_responses.len(), 0);
    }

    #[tokio::test]
    async fn test_server_run_no_response_when_no_alive_providers() {
        let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 12345);

        let request = DiscoveryRequest {
            allowed_providers: vec![ProviderType::Ollama as i32],
        };

        let mock_network = Arc::new(MockServerDiscoveryNetwork::new(vec![(
            request,
            client_addr,
        )]));

        // No alive providers
        let alive_providers = HashMap::new();

        let server =
            create_server_discovery_with_alive_providers(mock_network.clone(), alive_providers)
                .await;

        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        let sent_responses = mock_network.get_sent_responses().await;
        assert_eq!(sent_responses.len(), 0);
    }

    #[tokio::test]
    async fn test_server_run_handles_multiple_requests() {
        let client1_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 12345);
        let client2_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 60)), 12346);

        let request1 = DiscoveryRequest {
            allowed_providers: vec![ProviderType::Ollama as i32],
        };
        let request2 = DiscoveryRequest {
            allowed_providers: vec![ProviderType::Vllm as i32],
        };

        // Requests are popped, so order is reversed
        let mock_network = Arc::new(MockServerDiscoveryNetwork::new(vec![
            (request2, client2_addr),
            (request1, client1_addr),
        ]));

        let mut alive_providers = HashMap::new();
        alive_providers.insert(ProviderType::Ollama, 11434);
        alive_providers.insert(ProviderType::Vllm, 8000);

        let server =
            create_server_discovery_with_alive_providers(mock_network.clone(), alive_providers)
                .await;

        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        let sent_responses = mock_network.get_sent_responses().await;
        assert_eq!(sent_responses.len(), 2);

        // First response should be for client1 (Ollama)
        let (response1, addr1) = &sent_responses[0];
        assert_eq!(*addr1, client1_addr);
        assert_eq!(response1.provider_info.len(), 1);
        assert_eq!(
            response1.provider_info[0].provider_type,
            ProviderType::Ollama as i32
        );

        // Second response should be for client2 (VLLM)
        let (response2, addr2) = &sent_responses[1];
        assert_eq!(*addr2, client2_addr);
        assert_eq!(response2.provider_info.len(), 1);
        assert_eq!(
            response2.provider_info[0].provider_type,
            ProviderType::Vllm as i32
        );
    }

    /// Helper to create a UdpServerDiscovery with mock providers for liveness tests
    fn create_server_discovery_with_mock_providers(
        providers: HashMap<ProviderType, Arc<dyn Provider>>,
        allowed_providers: Vec<ProviderType>,
    ) -> UdpServerDiscovery {
        let mock_network = Arc::new(MockServerDiscoveryNetwork::new(vec![]));
        let proxy_ports = convert_providers_to_server_proxy_ports(&providers);

        UdpServerDiscovery::with_network(
            mock_network,
            providers,
            allowed_providers,
            proxy_ports,
            Duration::from_millis(10), // Short interval for tests
        )
    }

    #[tokio::test]
    async fn test_server_run_adds_healthy_provider() {
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            ProviderType::Ollama,
            Arc::new(MockProvider::always_healthy(11434)),
        );

        let server =
            create_server_discovery_with_mock_providers(providers, vec![ProviderType::Ollama]);

        // Initially no alive providers
        assert!(server.alive_providers.lock().await.is_empty());

        // Run server discovery with timeout (includes liveness check)
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        // Provider should now be alive
        let alive = server.alive_providers.lock().await;
        assert_eq!(alive.len(), 1);
        assert_eq!(alive.get(&ProviderType::Ollama), Some(&11434));
    }

    #[tokio::test]
    async fn test_server_run_removes_unhealthy_provider() {
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            ProviderType::Ollama,
            Arc::new(MockProvider::always_unhealthy(11434)),
        );

        let server =
            create_server_discovery_with_mock_providers(providers, vec![ProviderType::Ollama]);

        // Pre-populate with an alive provider
        server
            .alive_providers
            .lock()
            .await
            .insert(ProviderType::Ollama, 11434);

        // Run server discovery (includes liveness check)
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        // Provider should be removed
        let alive = server.alive_providers.lock().await;
        assert!(alive.is_empty());
    }

    #[tokio::test]
    async fn test_server_run_handles_health_check_error() {
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            ProviderType::Ollama,
            Arc::new(MockProvider::with_error(11434)),
        );

        let server =
            create_server_discovery_with_mock_providers(providers, vec![ProviderType::Ollama]);

        // Pre-populate with an alive provider
        server
            .alive_providers
            .lock()
            .await
            .insert(ProviderType::Ollama, 11434);

        // Run server discovery (includes liveness check)
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        // Provider should be removed on error
        let alive = server.alive_providers.lock().await;
        assert!(alive.is_empty());
    }

    #[tokio::test]
    async fn test_server_run_multiple_providers() {
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            ProviderType::Ollama,
            Arc::new(MockProvider::always_healthy(11434)),
        );
        providers.insert(
            ProviderType::Vllm,
            Arc::new(MockProvider::always_healthy(8000)),
        );
        providers.insert(
            ProviderType::LmStudio,
            Arc::new(MockProvider::always_unhealthy(1234)),
        );

        let server = create_server_discovery_with_mock_providers(
            providers,
            vec![
                ProviderType::Ollama,
                ProviderType::Vllm,
                ProviderType::LmStudio,
            ],
        );

        // Run server discovery (includes liveness check)
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        // Only healthy providers should be alive
        let alive = server.alive_providers.lock().await;
        assert_eq!(alive.len(), 2);
        assert_eq!(alive.get(&ProviderType::Ollama), Some(&11434));
        assert_eq!(alive.get(&ProviderType::Vllm), Some(&8000));
        assert!(!alive.contains_key(&ProviderType::LmStudio));
    }

    #[tokio::test]
    async fn test_server_run_only_checks_allowed_providers() {
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            ProviderType::Ollama,
            Arc::new(MockProvider::always_healthy(11434)),
        );
        providers.insert(
            ProviderType::Vllm,
            Arc::new(MockProvider::always_healthy(8000)),
        );

        // Only allow Ollama, not VLLM
        let server =
            create_server_discovery_with_mock_providers(providers, vec![ProviderType::Ollama]);

        // Run server discovery (includes liveness check)
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        // Only allowed provider should be checked and added
        let alive = server.alive_providers.lock().await;
        assert_eq!(alive.len(), 1);
        assert_eq!(alive.get(&ProviderType::Ollama), Some(&11434));
        assert!(!alive.contains_key(&ProviderType::Vllm));
    }

    #[tokio::test]
    async fn test_server_run_provider_state_transitions() {
        // Test that a provider can transition from unhealthy to healthy
        // The mock will return: false (first call), then true for all subsequent calls
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            ProviderType::Ollama,
            Arc::new(MockProvider::new(11434, vec![false, true])),
        );

        let server =
            create_server_discovery_with_mock_providers(providers, vec![ProviderType::Ollama]);

        // Initially not alive
        assert!(server.alive_providers.lock().await.is_empty());

        // Run server discovery - after multiple liveness check iterations, provider should be healthy
        // (first iteration: false -> not added, subsequent iterations: true -> added)
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        // Provider should be alive (added during one of the later iterations)
        let alive = server.alive_providers.lock().await;
        assert_eq!(alive.len(), 1);
        assert_eq!(alive.get(&ProviderType::Ollama), Some(&11434));
    }

    #[tokio::test]
    async fn test_server_run_provider_goes_offline() {
        // Test that a provider that goes offline is removed
        // The mock returns: true (first call), then false for all subsequent calls
        let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            ProviderType::Ollama,
            Arc::new(MockProvider::new(11434, vec![true, false])),
        );

        let server =
            create_server_discovery_with_mock_providers(providers, vec![ProviderType::Ollama]);

        // Initially not alive
        assert!(server.alive_providers.lock().await.is_empty());

        // Run server discovery - provider will be added then removed
        // (first iteration: true -> added, subsequent iterations: false -> removed)
        let _ = tokio::time::timeout(Duration::from_millis(50), server.run()).await;

        // Provider should not be alive (removed during later iterations)
        let alive = server.alive_providers.lock().await;
        assert!(alive.is_empty());
    }
}
