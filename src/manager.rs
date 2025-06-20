use std::{collections::VecDeque, net::SocketAddr, time::Duration};

use futures_util::StreamExt;
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::{AbortHandle, JoinHandle},
    time,
};
use tokio_stream::wrappers::IntervalStream;

use crate::{discovery::ClientDiscovery, ollama::Ollama, proxy::ClientProxy};
use log::{debug, error, info};

const DEFAULT_LIVENESS_INTERVAL: Duration = Duration::from_secs(10);

pub struct ActiveProxy {
    proxy: ClientProxy,
    server: SocketAddr,
    liveness_handle: AbortHandle,
}

pub struct Manager {
    servers: VecDeque<SocketAddr>,
    active_proxy: Option<ActiveProxy>,
    liveness_interval: std::time::Duration,
}

pub enum ManagerCommand {
    Add(SocketAddr),
    Remove(SocketAddr),
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            servers: VecDeque::new(),
            active_proxy: None,
            liveness_interval: DEFAULT_LIVENESS_INTERVAL,
        }
    }
}

impl Manager {
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let client_discovery = ClientDiscovery::default();

        let (cmd_tx, cmd_rx) = mpsc::channel::<ManagerCommand>(32);

        tokio::select! {
            val = self.handle_commands(cmd_rx, &cmd_tx) => val,
            val = client_discovery.run(&cmd_tx) => val
        }
    }

    async fn handle_commands(
        &mut self,
        mut cmd_rx: Receiver<ManagerCommand>,
        cmd_tx: &Sender<ManagerCommand>,
    ) -> anyhow::Result<()> {
        loop {
            if let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    ManagerCommand::Add(server) => self.handle_add_server(server, cmd_tx).await?,
                    ManagerCommand::Remove(server) => {
                        self.handle_remove_server(server, cmd_tx).await?
                    }
                }
            }
        }
    }

    async fn handle_remove_server(
        &mut self,
        server: SocketAddr,
        cmd_tx: &Sender<ManagerCommand>,
    ) -> anyhow::Result<()> {
        // Remove the server from the queue
        if self.servers.contains(&server) {
            self.servers.retain(|s| *s != server);
        }

        // Stop an active proxy if it is running and its server is the server to be removed
        if let Some(ActiveProxy {
            proxy,
            server: active_server,
            liveness_handle,
        }) = &self.active_proxy
        {
            if *active_server == server {
                proxy.stop(true).await;
                liveness_handle.abort();

                self.active_proxy = None;
            }
        }

        // Run and register a new active proxy for the first server in the queue
        if let Some(next) = self.servers.front() {
            let ollama = Self::ollama_for_server(*next)?;
            self.register_proxy(*next, ollama, cmd_tx).await?;
        }

        Ok(())
    }

    async fn handle_add_server(
        &mut self,
        server: SocketAddr,
        cmd_tx: &Sender<ManagerCommand>,
    ) -> anyhow::Result<()> {
        // Don't do anything for the already added server
        if !self.servers.contains(&server) {
            let ollama = Self::ollama_for_server(server)?;

            // Check if the server is proxying requests and has a running Ollama instance
            match ollama.get_version().await {
                Ok(_) => {
                    // Add new server to the end of queue
                    self.servers.push_back(server);

                    // Run and register a new active proxy if there is no running
                    if self.active_proxy.is_none() {
                        self.register_proxy(server, ollama, cmd_tx).await?;
                    }
                }
                Err(error) => {
                    error!("Ollana server {} returned an error: {}", server, error);
                }
            }
        }

        Ok(())
    }

    async fn register_proxy(
        &mut self,
        server: SocketAddr,
        ollama: Ollama,
        cmd_tx: &Sender<ManagerCommand>,
    ) -> anyhow::Result<()> {
        let mut client_proxy = ClientProxy::from_server_socket_addr(server)?;
        let (tx, rx) = tokio::sync::oneshot::channel();

        info!("Spawning an Ollana proxy for address {}", server);

        actix_web::rt::spawn(async move { client_proxy.run_server(tx).await });

        if let Ok(proxy) = rx.await {
            let liveness_handle = self
                .run_liveness_check(server, ollama, cmd_tx)
                .await?
                .abort_handle();

            self.active_proxy = Some(ActiveProxy {
                proxy,
                server,
                liveness_handle,
            });

            info!("Registered an Ollana proxy for address {}", server);
        }

        Ok(())
    }

    async fn run_liveness_check(
        &self,
        server: SocketAddr,
        ollama: Ollama,
        cmd_tx: &Sender<ManagerCommand>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let mut stream = IntervalStream::new(time::interval(self.liveness_interval));
        let cmd_tx = cmd_tx.clone();

        let handle = tokio::spawn(async move {
            while stream.next().await.is_some() {
                debug!("Executing liveness check for address {}", server);

                match ollama.get_version().await {
                    Ok(_) => (),
                    Err(_) => {
                        info!("Deregistering an Ollana proxy for address {}", server);

                        cmd_tx
                            .send(ManagerCommand::Remove(server))
                            .await
                            .unwrap_or(())
                    }
                }
            }
        });

        Ok(handle)
    }

    fn ollama_for_server(server: SocketAddr) -> anyhow::Result<Ollama> {
        Ollama::from_socket_addr(server).inspect_err(|error| {
            error!(
                "Couldn't create an Ollama instance for address {}: {}",
                server, error
            )
        })
    }
}
