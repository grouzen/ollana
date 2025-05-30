use std::{
    io,
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    time::Duration,
};

use futures_util::StreamExt;
use log::{debug, error, info};
use tokio::{net::UdpSocket, sync::mpsc::Sender, time};
use tokio_stream::wrappers::IntervalStream;

use crate::{
    constants::{self, OLLANA_SERVER_PROXY_DEFAULT_PORT},
    manager::ManagerCommand,
};

const PROTO_MAGIC_NUMBER: u32 = 0x4C414E41; // LANA
const RANDOM_UDP_PORT: u16 = 0;
const DEFAULT_BROADCAST_INTERVAL: Duration = Duration::from_secs(5);

pub struct ClientDiscovery {
    server_port: u16,
    broadcast_interval: std::time::Duration,
}

pub struct ServerDiscovery {
    port: u16,
}

impl Default for ClientDiscovery {
    fn default() -> Self {
        Self {
            server_port: constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            broadcast_interval: DEFAULT_BROADCAST_INTERVAL,
        }
    }
}

impl Default for ServerDiscovery {
    fn default() -> Self {
        Self {
            port: constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
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

        while let Some(_) = stream.next().await {
            if let Ok(len) = self.send(&socket).await {
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
        let mut buf: [u8; 4] = [0u8; 4];

        loop {
            if let Ok((len, addr)) = self.recv(&socket, &mut buf).await {
                debug!("Client discovery received {} bytes from {}", len, addr);

                let magic = u32::from_be_bytes(buf);

                if magic == PROTO_MAGIC_NUMBER {
                    debug!("Client discovery found a server with address {}", addr);

                    // TODO: replace default port with the actual server proxy port received from a server
                    let http_addr = (addr.ip(), OLLANA_SERVER_PROXY_DEFAULT_PORT)
                        .to_socket_addrs()?
                        .next()
                        .ok_or_else(|| {
                            anyhow::Error::msg("Server proxy address is invalid".to_string())
                        })?;

                    cmd_tx
                        .send(ManagerCommand::Add(http_addr))
                        .await
                        .unwrap_or(());
                } else {
                    let hex = format!("0X{:X}", magic);
                    debug!("Client discovery skipped message: {}", hex);
                }
            }
        }
    }

    async fn send(&self, socket: &UdpSocket) -> io::Result<usize> {
        socket
            .send_to(
                &PROTO_MAGIC_NUMBER.to_be_bytes(),
                (Ipv4Addr::BROADCAST, self.server_port),
            )
            .await
            .inspect_err(|error| error!("Client discovery error while sending: {}", error))
    }

    async fn recv(&self, socket: &UdpSocket, buf: &mut [u8; 4]) -> io::Result<(usize, SocketAddr)> {
        socket
            .recv_from(buf)
            .await
            .inspect_err(|error| error!("Server discovery error while receiving: {}", error))
    }
}

impl ServerDiscovery {
    pub async fn run(&self) -> anyhow::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, self.port)).await?;
        let local_addr = socket.local_addr()?;
        let mut buf: [u8; 4] = [0u8; 4];

        info!("Running server discovery on {}...", local_addr);

        loop {
            if let Ok((len, addr)) = self.recv(&socket, &mut buf).await {
                debug!("Server discovery received {} bytes from {}", len, addr);

                let magic = u32::from_be_bytes(buf);

                if magic == PROTO_MAGIC_NUMBER {
                    if let Ok(len) = self.send(&socket, addr).await {
                        debug!("Server discovery sent {} bytes to {}", len, addr);
                    }
                } else {
                    let hex = format!("0X{:X}", magic);
                    debug!("Server discovery skipped message: {}", hex);
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
