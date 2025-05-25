use std::{io, net::Ipv4Addr, time::Duration};

use futures_util::StreamExt;
use log::{error, info};
use tokio::{net::UdpSocket, time};
use tokio_stream::wrappers::IntervalStream;

use crate::constants;

const PROTO_MAGIC_NUMBER: u32 = 0x4C414E41; // LANA
const RANDOM_UDP_PORT: u16 = 0;
const DEFAULT_BROADCAST_INTERVAL: Duration = Duration::from_secs(30);

pub struct ClientDiscovery {
    server_port: u16,
    broadcast_interval: std::time::Duration,
}

impl Default for ClientDiscovery {
    fn default() -> Self {
        Self {
            server_port: constants::OLLANA_SERVER_DEFAULT_DISCOVERY_PORT,
            broadcast_interval: DEFAULT_BROADCAST_INTERVAL,
        }
    }
}

impl ClientDiscovery {
    pub async fn run(&self) -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, RANDOM_UDP_PORT)).await?;
        let mut stream = IntervalStream::new(time::interval(self.broadcast_interval));

        socket.set_broadcast(true)?;

        while let Some(_) = stream.next().await {
            match self.send(&socket).await {
                Ok(len) => {
                    info!("Client discovery sent {} bytes", len);
                }
                Err(error) => {
                    error!("Client discovery error {}", error);
                }
            };
        }

        Ok(())
    }

    async fn send(&self, socket: &UdpSocket) -> io::Result<usize> {
        socket
            .send_to(
                &PROTO_MAGIC_NUMBER.to_be_bytes(),
                (Ipv4Addr::BROADCAST, self.server_port),
            )
            .await
    }
}
