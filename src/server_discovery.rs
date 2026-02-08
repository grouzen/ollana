use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, SocketAddr},
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
    constants, create_default_proto_providers,
    proto::{DiscoveryRequest, DiscoveryResponse, ProviderInfo, ProviderType},
    provider::Provider,
    server_manager::ServerManagerCommand,
    ALL_PROTO_PROVIDER_TYPES,
};

const DEFAULT_SERVER_LIVENESS_INTERVAL: Duration = Duration::from_secs(10);
const DISCOVERY_BUFFER_SIZE: usize = 1024;

pub fn convert_providers_to_server_proxy_ports(
    providers: &HashMap<ProviderType, Arc<dyn Provider>>,
) -> HashMap<ProviderType, u16> {
    let mut proxy_ports = HashMap::new();
    for (provider_type, provider) in providers {
        proxy_ports.insert(*provider_type, provider.get_port() + 1);
    }
    proxy_ports
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

/// Trait for server-side discovery operations.
/// This allows for different implementations of server discovery behavior.
#[async_trait]
pub trait ServerDiscovery: Send + Sync {
    /// Run the server discovery process.
    async fn run(&self, cmd_tx: &Sender<ServerManagerCommand>) -> anyhow::Result<()>;
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
        let providers = create_default_proto_providers();
        let proxy_ports = convert_providers_to_server_proxy_ports(&providers);

        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            providers,
            ALL_PROTO_PROVIDER_TYPES.to_vec(),
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

    /// Periodically check the liveness of providers and send commands to ServerManager.
    async fn run_liveness_check(&self, cmd_tx: &Sender<ServerManagerCommand>) {
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

                                // Notify ServerManager to start proxy for this provider
                                cmd_tx
                                    .send(ServerManagerCommand::Start(
                                        *provider_type,
                                        provider_port,
                                    ))
                                    .await
                                    .unwrap_or_else(|e| {
                                        error!(
                                            "Failed to send Start command to ServerManager: {}",
                                            e
                                        );
                                    });
                            }
                        }
                        Ok(false) | Err(_) => {
                            if let Some(provider_port) = alive_providers.remove(provider_type) {
                                info!(
                                    "Detected {:?} on port {} is no longer running, will stop responding for this provider",
                                    provider_type, provider_port
                                );

                                // Notify ServerManager to stop proxy for this provider
                                cmd_tx
                                    .send(ServerManagerCommand::Stop(*provider_type))
                                    .await
                                    .unwrap_or_else(|e| {
                                        error!(
                                            "Failed to send Stop command to ServerManager: {}",
                                            e
                                        );
                                    });
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
    async fn run(&self, cmd_tx: &Sender<ServerManagerCommand>) -> anyhow::Result<()> {
        info!("Running server discovery...");

        tokio::select! {
            val = self.handle_messages() => val,
            val = self.run_liveness_check(cmd_tx) => Ok(val),
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

    /// Helper to create a dummy command sender for tests
    fn create_dummy_cmd_tx() -> Sender<ServerManagerCommand> {
        let (tx, _rx) = mpsc::channel::<ServerManagerCommand>(32);
        tx
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
            ALL_PROTO_PROVIDER_TYPES.to_vec(),
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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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

        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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

        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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

        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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

        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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

        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

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
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            server.run(&create_dummy_cmd_tx()),
        )
        .await;

        // Provider should not be alive (removed during later iterations)
        let alive = server.alive_providers.lock().await;
        assert!(alive.is_empty());
    }
}
