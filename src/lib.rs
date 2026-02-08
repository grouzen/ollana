use std::{collections::HashMap, path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::provider::{LMStudio, LlamaServer, Ollama, Provider, VLLM};

pub mod args;
pub mod certs;
pub mod client_discovery;
pub mod client_manager;
pub mod client_proxy;
pub mod config;
pub mod constants;
pub mod device;
pub mod ollana;
pub mod provider;
pub mod serve_app;
pub mod server_discovery;
pub mod server_health_monitor;
pub mod server_manager;
pub mod server_proxy;

// Include generated protobuf code
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/ollana.discovery.rs"));
}

pub const HTTP_HEADER_OLLANA_DEVICE_ID: &str = "X-Ollana-Device-Id";

/// All provider types in consistent order
pub const ALL_PROVIDER_TYPES: &[ProviderType] = &[
    ProviderType::Ollama,
    ProviderType::Vllm,
    ProviderType::LmStudio,
    ProviderType::LlamaServer,
];

pub const ALL_PROTO_PROVIDER_TYPES: &[proto::ProviderType] = &[
    proto::ProviderType::Ollama,
    proto::ProviderType::Vllm,
    proto::ProviderType::LmStudio,
    proto::ProviderType::LlamaServer,
];

pub enum Mode {
    Client,
    Server,
}

/// Returns the path to the local data directory used by Ollana.
///
/// This method attempts to determine the location of the application's local data directory using
/// the `dirs` crate. If successful, it returns a `PathBuf` pointing to a subdirectory named
/// "ollana" within this directory.
///
/// # Returns
/// A `Result<PathBuf>` indicating success or failure:
/// - Ok(PathBuf): The path to the local data directory for Ollana.
/// - Err(anyhow::Error): An error if the local data directory cannot be determined.
///
/// # Errors
/// This function can return an `anyhow::Error` if it fails to determine the data local directory.
///
pub fn get_local_dir() -> anyhow::Result<PathBuf> {
    dirs::data_local_dir()
        .map(|p| p.join("ollana"))
        .ok_or(anyhow::Error::msg(
            "Couldn't determine data local directory",
        ))
}

pub fn create_default_providers() -> HashMap<ProviderType, Arc<dyn Provider>> {
    let mut providers: HashMap<ProviderType, Arc<dyn Provider>> = HashMap::new();

    providers.insert(ProviderType::Ollama, Arc::new(Ollama::default()));
    providers.insert(ProviderType::Vllm, Arc::new(VLLM::default()));
    providers.insert(ProviderType::LmStudio, Arc::new(LMStudio::default()));
    providers.insert(ProviderType::LlamaServer, Arc::new(LlamaServer::default()));

    providers
}

pub fn create_default_proto_providers() -> HashMap<proto::ProviderType, Arc<dyn Provider>> {
    let providers = create_default_providers();

    providers
        .into_iter()
        .map(|(k, v)| (k.into(), v.clone()))
        .collect()
}

/// Parse a port number string into u16
fn parse_port(s: &str) -> Result<u16, String> {
    s.parse::<u16>()
        .map_err(|_| format!("Invalid port number: {s}. Must be between 0 and 65535"))
}

/// Port mapping configuration for a provider
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortMapping {
    /// First port (server: LLM port, client: server proxy port)
    pub port1: Option<u16>,
    /// Second port (server: server proxy port, client: client proxy port)
    pub port2: Option<u16>,
}

impl Serialize for PortMapping {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as string format "port1:port2"
        let s = match (self.port1, self.port2) {
            (Some(p1), Some(p2)) => format!("{p1}:{p2}"),
            (Some(p1), None) => format!("{p1}:"),
            (None, Some(p2)) => format!(":{p2}"),
            (None, None) => ":".to_string(),
        };
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for PortMapping {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl std::str::FromStr for PortMapping {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid port mapping format: {}. Expected <port1>:<port2>, <port1>: or :<port2>",
                s
            ));
        }

        match (parts[0].is_empty(), parts[1].is_empty()) {
            // :<port2> format
            (true, false) => {
                let port2 = parse_port(parts[1])?;
                Ok(PortMapping {
                    port1: None,
                    port2: Some(port2),
                })
            }
            // <port1>: format
            (false, true) => {
                let port1 = parse_port(parts[0])?;
                Ok(PortMapping {
                    port1: Some(port1),
                    port2: None,
                })
            }
            // <port1>:<port2> format
            (false, false) => {
                let port1 = parse_port(parts[0])?;
                let port2 = parse_port(parts[1])?;
                Ok(PortMapping {
                    port1: Some(port1),
                    port2: Some(port2),
                })
            }
            // Invalid format ":"
            (true, true) => Err(format!(
                "Invalid port mapping format: {}. Expected <port1>:<port2>, <port1>: or :<port2>",
                s
            )),
        }
    }
}

/// Provider type enumeration matching the protobuf definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderType {
    Ollama,
    Vllm,
    LmStudio,
    LlamaServer,
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::Ollama => write!(f, "ollama"),
            ProviderType::Vllm => write!(f, "vllm"),
            ProviderType::LmStudio => write!(f, "lm-studio"),
            ProviderType::LlamaServer => write!(f, "llama-server"),
        }
    }
}

impl std::str::FromStr for ProviderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ollama" => Ok(ProviderType::Ollama),
            "vllm" => Ok(ProviderType::Vllm),
            "lm-studio" => Ok(ProviderType::LmStudio),
            "llama-server" => Ok(ProviderType::LlamaServer),
            _ => Err(format!(
                "Invalid provider type: {}. Valid values: ollama, vllm, lm-studio, llama-server",
                s
            )),
        }
    }
}

impl From<ProviderType> for proto::ProviderType {
    fn from(provider: ProviderType) -> Self {
        match provider {
            ProviderType::Ollama => proto::ProviderType::Ollama,
            ProviderType::Vllm => proto::ProviderType::Vllm,
            ProviderType::LmStudio => proto::ProviderType::LmStudio,
            ProviderType::LlamaServer => proto::ProviderType::LlamaServer,
        }
    }
}

impl From<proto::ProviderType> for ProviderType {
    fn from(provider: proto::ProviderType) -> Self {
        match provider {
            proto::ProviderType::Ollama => ProviderType::Ollama,
            proto::ProviderType::Vllm => ProviderType::Vllm,
            proto::ProviderType::LmStudio => ProviderType::LmStudio,
            proto::ProviderType::LlamaServer => ProviderType::LlamaServer,
            proto::ProviderType::Unspecified => ProviderType::Ollama, // Default fallback
        }
    }
}
