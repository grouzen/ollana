use std::{collections::VecDeque, net::SocketAddr};

use tokio::sync::mpsc::{self, Receiver};

use crate::{discovery::ClientDiscovery, ollama::Ollama, proxy::ClientProxy};
use log::{error, info};

#[derive(Default)]
pub struct Manager {
    servers: VecDeque<SocketAddr>,
    active_proxy: Option<ClientProxy>,
}

pub enum ManagerCommand {
    Add(SocketAddr),
    Remove(SocketAddr),
}

impl Manager {
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let client_discovery = ClientDiscovery::default();

        let (cmd_tx, cmd_rx) = mpsc::channel::<ManagerCommand>(32);

        tokio::select! {
            val = self.handle_commands(cmd_rx) => val,
            val = client_discovery.run(cmd_tx) => val
        }
    }

    async fn handle_commands(
        &mut self,
        mut cmd_rx: Receiver<ManagerCommand>,
    ) -> anyhow::Result<()> {
        loop {
            if let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    ManagerCommand::Add(server) => self.handle_add_server(server).await?,
                    ManagerCommand::Remove(_) => (),
                }
            }
        }
    }

    async fn handle_add_server(&mut self, server: SocketAddr) -> anyhow::Result<()> {
        if !self.servers.contains(&server) {
            let ollama = Ollama::from_socket_addr(server).inspect_err(|error| {
                error!(
                    "Couldn't create an Ollama instance for address {}: {}",
                    server, error
                )
            })?;

            match ollama.get_version().await {
                Ok(_) => {
                    self.servers.push_back(server);

                    if let None = self.active_proxy {
                        self.register_proxy(server).await?;
                    }
                }
                Err(error) => {
                    error!("Ollana server {} returned an error: {}", server, error);
                }
            }
        }

        Ok(())
    }

    async fn register_proxy(&mut self, server: SocketAddr) -> anyhow::Result<()> {
        let mut client_proxy = ClientProxy::from_server_socket_addr(server)?;
        let (tx, rx) = tokio::sync::oneshot::channel();

        info!("Spawning an Ollana proxy for address {}", server);

        actix_web::rt::spawn(async move { client_proxy.run_server(tx).await });

        if let Ok(proxy) = rx.await {
            self.active_proxy = Some(proxy);
            info!("Registered an Ollana proxy for address {}", server);
        }

        Ok(())
    }
}
