use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

use futures_util::StreamExt;
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::{AbortHandle, JoinHandle},
    time,
};
use tokio_stream::wrappers::IntervalStream;

use crate::{
    device::Device,
    discovery::{ClientDiscovery, UdpClientDiscovery},
    ollana::{HttpOllana, Ollana},
    proto::ProviderType,
    provider::{Ollama, Provider},
    proxy::ClientProxy,
};
use log::{debug, error, info};

const DEFAULT_LIVENESS_INTERVAL: Duration = Duration::from_secs(10);

pub struct ActiveProxy {
    proxy: Box<dyn ClientProxy>,
    server_socket_addr: SocketAddr,
    liveness_handle: AbortHandle,
}

pub struct ClientManager<F>
where
    F: Fn(ProviderType, SocketAddr, Arc<dyn Device>) -> anyhow::Result<Box<dyn ClientProxy>>
        + Send
        + Sync,
{
    server_queues: HashMap<ProviderType, VecDeque<SocketAddr>>,
    active_proxies: HashMap<ProviderType, ActiveProxy>,
    liveness_interval: std::time::Duration,
    device: Arc<dyn Device>,
    proxy_factory: F,
    allowed_providers: Vec<ProviderType>,
}

pub enum ClientManagerCommand {
    Add(ProviderType, SocketAddr),
    Remove(ProviderType, SocketAddr),
}

impl<F> ClientManager<F>
where
    F: Fn(ProviderType, SocketAddr, Arc<dyn Device>) -> anyhow::Result<Box<dyn ClientProxy>>
        + Send
        + Sync,
{
    pub fn new(
        device: Arc<dyn Device>,
        proxy_factory: F,
        allowed_providers: Vec<ProviderType>,
    ) -> Self {
        Self {
            server_queues: HashMap::new(),
            active_proxies: HashMap::new(),
            liveness_interval: DEFAULT_LIVENESS_INTERVAL,
            device,
            proxy_factory,
            allowed_providers,
        }
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        let client_discovery =
            UdpClientDiscovery::with_allowed_providers(self.allowed_providers.clone()).await?;

        let (cmd_tx, cmd_rx) = mpsc::channel::<ClientManagerCommand>(32);

        tokio::select! {
            val = self.handle_commands(cmd_rx, &cmd_tx) => val,
            val = client_discovery.run(&cmd_tx) => val
        }
    }

    async fn handle_commands(
        &mut self,
        mut cmd_rx: Receiver<ClientManagerCommand>,
        cmd_tx: &Sender<ClientManagerCommand>,
    ) -> anyhow::Result<()> {
        loop {
            if let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    ClientManagerCommand::Add(provider_type, server_socket_addr) => {
                        self.handle_add_server(provider_type, server_socket_addr, cmd_tx)
                            .await?
                    }
                    ClientManagerCommand::Remove(provider_type, server_socket_addr) => {
                        self.handle_remove_server(provider_type, server_socket_addr, cmd_tx)
                            .await?
                    }
                }
            }
        }
    }

    async fn handle_remove_server(
        &mut self,
        provider_type: ProviderType,
        server_socket_addr: SocketAddr,
        cmd_tx: &Sender<ClientManagerCommand>,
    ) -> anyhow::Result<()> {
        // Remove the server from the provider queue
        if let Some(server_queue) = self.server_queues.get_mut(&provider_type) {
            server_queue.retain(|queued_server_socket_addr| {
                queued_server_socket_addr != &server_socket_addr
            });
        }

        // Stop an active proxy if it is running and its server/provider matches
        if let Some(active_proxy) = self.active_proxies.get(&provider_type) {
            if active_proxy.server_socket_addr == server_socket_addr {
                active_proxy.proxy.stop(true).await;
                active_proxy.liveness_handle.abort();

                self.active_proxies.remove(&provider_type);
            }
        }

        // Run and register a new active proxy for the first server in the queue for this provider
        if let Some(server_queue) = self.server_queues.get(&provider_type) {
            if let Some(next_server_queue_addr) = server_queue.front() {
                let ollama = Self::ollama_for_server(*next_server_queue_addr)?;

                self.register_proxy(provider_type, *next_server_queue_addr, ollama, cmd_tx)
                    .await?;
            }
        }

        Ok(())
    }

    /// Handles adding a new server to the manager.
    ///
    /// This method checks whether the provided provider instance is already in the list of managed servers,
    /// then proceeds to authenticate with the Ollama service at that address. If successful, it adds
    /// the server to the end of the provider's queue and registers a proxy if there isn't one currently active.
    ///
    /// # Arguments
    /// * `self` - A mutable reference to the manager instance.
    /// * `provider_type` - The type of provider being added.
    /// * `instance` - The server instance containing server address and port.
    /// * `cmd_tx` - A sender for sending commands to the manager (`&Sender<ClientManagerCommand>`).
    ///
    /// # Returns
    /// This function returns an `anyhow::Result<()>`, indicating success or failure.
    ///
    /// # Errors
    /// This method can return errors if any of the following occur:
    /// - The provided server address is not authorized.
    /// - There is an error in connecting to the Ollama service at the provided address.
    /// - Other unexpected issues arise during execution.
    ///
    async fn handle_add_server(
        &mut self,
        provider_type: ProviderType,
        server_socket_addr: SocketAddr,
        cmd_tx: &Sender<ClientManagerCommand>,
    ) -> anyhow::Result<()> {
        // Get or create the queue for this provider type
        let server_queue = self.server_queues.entry(provider_type).or_default();

        // Don't do anything for the already added server
        let already_exists = server_queue
            .iter()
            .any(|queued_server_socket_addr| *queued_server_socket_addr == server_socket_addr);

        if !already_exists {
            let ollama = Self::ollama_for_server(server_socket_addr)?;
            let ollana = HttpOllana::new(server_socket_addr)?;

            if let Some(auth_response) = ollana.check_authorization(self.device.get_id()).await? {
                let server_device_id = auth_response.device_id;

                // Check if the server's device_id is allowed on the client
                if self.device.is_allowed(server_device_id.clone()) {
                    // Check if the server is proxying requests and has a running Ollama instance
                    match ollama.health_check().await {
                        Ok(_) => {
                            // Add new server to the end of queue for this provider
                            server_queue.push_back(server_socket_addr);

                            // Run and register a new active proxy if there is no running proxy for this provider
                            if !self.active_proxies.contains_key(&provider_type) {
                                self.register_proxy(
                                    provider_type,
                                    server_socket_addr,
                                    ollama,
                                    cmd_tx,
                                )
                                .await?;
                            }
                        }
                        Err(error) => {
                            error!(
                                "Ollana server {} returned an error: {}",
                                server_socket_addr, error
                            );
                        }
                    }
                } else {
                    debug!(
                        "Ollana server is not allowed to be registered: {}",
                        server_device_id
                    );
                }
            }
        }

        Ok(())
    }

    async fn register_proxy(
        &mut self,
        provider_type: ProviderType,
        server_socket_addr: SocketAddr,
        ollama: Ollama,
        cmd_tx: &Sender<ClientManagerCommand>,
    ) -> anyhow::Result<()> {
        let mut client_proxy =
            (self.proxy_factory)(provider_type, server_socket_addr, self.device.clone())?;
        let (tx, rx) = tokio::sync::oneshot::channel();

        info!(
            "Spawning an Ollana proxy for address {}",
            server_socket_addr
        );

        actix_web::rt::spawn(async move { client_proxy.run_server(tx).await });

        if let Ok(proxy) = rx.await {
            let liveness_handle = self
                .run_liveness_check(provider_type, server_socket_addr, ollama, cmd_tx)
                .await?
                .abort_handle();

            self.active_proxies.insert(
                provider_type,
                ActiveProxy {
                    proxy,
                    server_socket_addr,
                    liveness_handle,
                },
            );

            info!(
                "Registered an Ollana proxy for address {}",
                server_socket_addr
            );
        }

        Ok(())
    }

    async fn run_liveness_check(
        &self,
        provider_type: ProviderType,
        server_socket_addr: SocketAddr,
        ollama: Ollama,
        cmd_tx: &Sender<ClientManagerCommand>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let mut stream = IntervalStream::new(time::interval(self.liveness_interval));
        let cmd_tx = cmd_tx.clone();

        let handle = tokio::spawn(async move {
            while stream.next().await.is_some() {
                debug!(
                    "Executing liveness check for address {}",
                    server_socket_addr
                );

                match ollama.health_check().await {
                    Ok(_) => (),
                    Err(_) => {
                        info!(
                            "Deregistering an Ollana proxy for address {}",
                            server_socket_addr
                        );

                        cmd_tx
                            .send(ClientManagerCommand::Remove(
                                provider_type,
                                server_socket_addr,
                            ))
                            .await
                            .unwrap_or(())
                    }
                }
            }
        });

        Ok(handle)
    }

    fn ollama_for_server(server_socket_addr: SocketAddr) -> anyhow::Result<Ollama> {
        Ollama::new(server_socket_addr, true).inspect_err(|error| {
            error!(
                "Couldn't create an Ollama instance for address {}: {}",
                server_socket_addr, error
            )
        })
    }
}
