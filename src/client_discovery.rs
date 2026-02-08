use std::{
    io,
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use log::{debug, error, info};
use prost::Message;
use tokio::{net::UdpSocket, sync::mpsc::Sender, time};
use tokio_stream::wrappers::IntervalStream;

use crate::{
    client_manager::ClientManagerCommand,
    constants,
    proto::{DiscoveryRequest, DiscoveryResponse, ProviderType},
};

const RANDOM_UDP_PORT: u16 = 0;
const DEFAULT_CLIENT_BROADCAST_INTERVAL: Duration = Duration::from_secs(5);
const DISCOVERY_BUFFER_SIZE: usize = 1024;

pub const DEFAULT_ALLOWED_PROVIDERS: &[ProviderType] = &[
    ProviderType::Ollama,
    ProviderType::Vllm,
    ProviderType::LmStudio,
    ProviderType::LlamaServer,
];

/// Trait for abstracting network operations in ClientDiscovery.
/// This allows for mocking network behavior in tests.
#[async_trait]
pub trait ClientDiscoveryNetwork: Send + Sync {
    /// Send a discovery request to the broadcast address.
    async fn send(&self, request: &DiscoveryRequest) -> io::Result<usize>;

    /// Receive a discovery response from the network.
    async fn recv(&self) -> io::Result<(DiscoveryResponse, SocketAddr)>;
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

/// Trait for client-side discovery operations.
/// This allows for different implementations of client discovery behavior.
#[async_trait]
pub trait ClientDiscovery: Send + Sync {
    /// Run the client discovery process.
    async fn run(&self, cmd_tx: &Sender<ClientManagerCommand>) -> anyhow::Result<()>;
}

/// UDP-based implementation of ClientDiscovery.
pub struct UdpClientDiscovery {
    network: Arc<dyn ClientDiscoveryNetwork>,
    broadcast_interval: std::time::Duration,
    allowed_providers: Vec<ProviderType>,
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

    pub async fn with_allowed_providers(allowed_providers: Vec<ProviderType>) -> io::Result<Self> {
        Self::new(
            constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            DEFAULT_CLIENT_BROADCAST_INTERVAL,
            allowed_providers,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::ProviderInfo;
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::sync::{mpsc, Mutex};

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
}
