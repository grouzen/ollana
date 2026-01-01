use std::{
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
    ollama::Ollama,
    proto::{DiscoveryRequest, DiscoveryResponse, ProviderType},
};

// Temporary: Keep for ServerDiscovery until it's refactored
const PROTO_MAGIC_NUMBER: u32 = 0x4C414E41; // LANA
const RANDOM_UDP_PORT: u16 = 0;
const DEFAULT_CLIENT_BROADCAST_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_SERVER_LIVENESS_INTERVAL: Duration = Duration::from_secs(10);
const DISCOVERY_BUFFER_SIZE: usize = 1024;

pub struct ClientDiscovery {
    server_port: u16,
    broadcast_interval: std::time::Duration,
    allowed_providers: Vec<ProviderType>,
}

pub struct ServerDiscovery {
    port: u16,
    local_ollama: Arc<Ollama>,
    liveness_interval: std::time::Duration,
    alive: Mutex<bool>,
}

impl Default for ClientDiscovery {
    fn default() -> Self {
        Self {
            server_port: constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            broadcast_interval: DEFAULT_CLIENT_BROADCAST_INTERVAL,
            allowed_providers: vec![
                ProviderType::Ollama,
                ProviderType::Vllm,
                ProviderType::LmStudio,
                ProviderType::LlamaServer,
            ],
        }
    }
}

impl Default for ServerDiscovery {
    fn default() -> Self {
        Self {
            port: constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            local_ollama: Arc::new(Ollama::default()),
            liveness_interval: DEFAULT_SERVER_LIVENESS_INTERVAL,
            alive: Mutex::new(true),
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
            if let Ok(len) = self.send(socket).await {
                debug!("Client discovery sent {} bytes", len);
            }
        }

        Ok(())
    }

    async fn handle_messages(
        &self,
        socket: &UdpSocket,
        cmd_tx: &Sender<ManagerCommand>,
    ) -> anyhow::Result<()> {
        let mut buf = vec![0u8; DISCOVERY_BUFFER_SIZE];

        loop {
            if let Ok((len, addr)) = self.recv(socket, &mut buf).await {
                debug!("Client discovery received {} bytes from {}", len, addr);

                match DiscoveryResponse::decode(&buf[..len]) {
                    Ok(response) => {
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
                    Err(e) => {
                        debug!("Client discovery failed to decode message: {}", e);
                    }
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

        socket
            .send_to(&buf, (Ipv4Addr::BROADCAST, self.server_port))
            .await
            .inspect_err(|error| error!("Client discovery error while sending: {}", error))
    }

    async fn recv(&self, socket: &UdpSocket, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        socket
            .recv_from(buf)
            .await
            .inspect_err(|error| error!("Client discovery error while receiving: {}", error))
    }
}

impl ServerDiscovery {
    pub fn new(local_ollama: Arc<Ollama>) -> Self {
        Self {
            local_ollama,
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
        let mut buf: [u8; 4] = [0u8; 4];

        loop {
            if let Ok((len, addr)) = self.recv(socket, &mut buf).await {
                let alive = self.alive.lock().await;

                if *alive {
                    debug!("Server discovery received {} bytes from {}", len, addr);

                    let magic = u32::from_be_bytes(buf);

                    if magic == PROTO_MAGIC_NUMBER {
                        if let Ok(len) = self.send(socket, addr).await {
                            debug!("Server discovery sent {} bytes to {}", len, addr);
                        }
                    } else {
                        let hex = format!("0X{:X}", magic);
                        debug!("Server discovery skipped message: {}", hex);
                    }
                }
            }
        }
    }

    async fn run_liveness_check(&self) {
        let mut stream = IntervalStream::new(time::interval(self.liveness_interval));

        while stream.next().await.is_some() {
            debug!("Executing liveness check for locally running Ollama");

            let mut alive = self.alive.lock().await;

            match self.local_ollama.get_version().await {
                Ok(_) => {
                    if !*alive {
                        info!("Detected local Ollama is running, start responding to discovery messages");

                        *alive = true;
                    }
                }
                Err(_) => {
                    if *alive {
                        info!("Detected local Ollama is not running, stop responding to discovery messages");

                        *alive = false;
                    }
                }
            }
        }
    }

    async fn recv(&self, socket: &UdpSocket, buf: &mut [u8; 4]) -> io::Result<(usize, SocketAddr)> {
        socket
            .recv_from(buf)
            .await
            .inspect_err(|error| error!("Server discovery error while receiving: {}", error))
    }

    async fn send(&self, socket: &UdpSocket, addr: SocketAddr) -> io::Result<usize> {
        socket
            .send_to(&PROTO_MAGIC_NUMBER.to_be_bytes(), addr)
            .await
            .inspect_err(|error| error!("Server discovery error while sending: {}", error))
    }
}
