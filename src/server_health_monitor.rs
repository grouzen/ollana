use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
// use futures_util::StreamExt;
use futures_util::StreamExt as _;
use log::{debug, error, info};
use tokio::{sync::broadcast::Sender, time};
use tokio_stream::wrappers::IntervalStream;

use crate::{create_default_proto_providers, proto::ProviderType, provider::Provider};

pub enum ServerHealthMonitorEvent {
    ProviderWentOnline {
        provider_type: ProviderType,
        provider_port: u16,
    },
    ProviderWentOffline {
        provider_type: ProviderType,
        provider_port: u16,
    },
}

#[async_trait]
pub trait ServerHealthMonitor: Send + Sync {
    async fn run(&self, event_tx: Sender<ServerHealthMonitorEvent>);
}

pub struct DefaultServerHealthMonitor {
    providers: HashMap<ProviderType, Arc<dyn Provider>>,
    allowed_providers: Vec<ProviderType>,
    alive_providers: Arc<HashMap<ProviderType, u16>>,
    liveness_interval: Duration,
}

impl DefaultServerHealthMonitor {
    pub fn new(liveness_interval: Duration, allowed_providers: Vec<ProviderType>) -> Self {
        let providers = create_default_proto_providers();
        let alive_providers = Arc::new(HashMap::new());

        Self {
            providers,
            allowed_providers,
            alive_providers,
            liveness_interval,
        }
    }
}

#[async_trait]
impl ServerHealthMonitor for DefaultServerHealthMonitor {
    async fn run(&self, event_tx: Sender<ServerHealthMonitorEvent>) {
        let mut stream = IntervalStream::new(time::interval(self.liveness_interval));

        while stream.next().await.is_some() {
            debug!("Executing liveness checks for all allowed providers");

            for provider_type in &self.allowed_providers {
                if let Some(provider) = self.providers.get(&provider_type) {
                    let provider_port = provider.get_port();

                    match provider.health_check().await {
                        Ok(true) => {
                            if !self.alive_providers.contains_key(&provider_type) {
                                info!(
                                    "Detected {:?} is running on port {}, will start responding for this provider",
                                    provider_type, provider_port
                                );

                                let event = ServerHealthMonitorEvent::ProviderWentOnline {
                                    provider_type: *provider_type,
                                    provider_port,
                                };

                                // Notify ServerManager to start proxy for this provider
                                if let Err(e) = event_tx.send(event) {
                                    error!("Failed to send health event: {e}");
                                }
                            }
                        }
                        Ok(false) | Err(_) => {
                            info!(
                                    "Detected {:?} on port {} is no longer running, will stop responding for this provider",
                                    provider_type, provider_port
                                );

                            let event = ServerHealthMonitorEvent::ProviderWentOffline {
                                provider_type: *provider_type,
                                provider_port,
                            };

                            // Notify ServerManager to stop proxy for this provider
                            if let Err(e) = event_tx.send(event) {
                                error!("Failed to send health event: {e}");
                            }
                        }
                    }
                }
            }
        }
    }
}
