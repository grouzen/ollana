use std::collections::HashMap;

use clap::Parser;

use crate::{PortMapping, ProviderType};

#[derive(Parser)]
#[command(name = "ollana")]
#[command(bin_name = "ollana")]
#[command(version, about)]
pub enum Args {
    /// Run the ollana server
    Serve(ServeArgs),
    #[clap(subcommand)]
    /// Manage devices
    Device(DeviceCommands),
}

#[derive(clap::Args)]
pub struct ServeArgs {
    #[arg(
        short = 'd',
        long,
        default_value_t = false,
        help = "Run in daemon mode"
    )]
    pub daemon: bool,
    #[arg(
        long = "pid",
        value_name = "PID_FILE",
        help = "PID file path (only valid when --daemon is used)",
        required = false,
        requires = "daemon"
    )]
    pub pid_file: Option<std::path::PathBuf>,
    #[arg(
        long = "log-file",
        value_name = "LOG_FILE",
        help = "Log file path",
        required = false
    )]
    pub log_file: Option<std::path::PathBuf>,
    #[arg(
        long = "force-server-mode",
        default_value_t = false,
        help = "Force server mode regardless of Ollama availability (useful for boot order issues)"
    )]
    pub force_server_mode: bool,
    #[arg(
        long = "allowed-providers",
        value_name = "PROVIDERS",
        help = "Comma-separated list of allowed provider types (ollama, vllm, lm-studio, llama-server)",
        value_delimiter = ',',
        required = false
    )]
    pub allowed_providers: Option<Vec<ProviderType>>,
    #[arg(
        long = "ollama-ports",
        value_name = "PORT_MAPPING",
        help = "Port mapping: <port1>:<port2>, <port1>: or :<port2>",
        required = false
    )]
    pub ollama_ports: Option<PortMapping>,
    #[arg(
        long = "vllm-ports",
        value_name = "PORT_MAPPING",
        help = "Port mapping: <port1>:<port2>, <port1>: or :<port2>",
        required = false
    )]
    pub vllm_ports: Option<PortMapping>,
    #[arg(
        long = "lmstudio-ports",
        value_name = "PORT_MAPPING",
        help = "Port mapping: <port1>:<port2>, <port1>: or :<port2>",
        required = false
    )]
    pub lmstudio_ports: Option<PortMapping>,
    #[arg(
        long = "llama-server-ports",
        value_name = "PORT_MAPPING",
        help = "Port mapping: <port1>:<port2>, <port1>: or :<port2>",
        required = false
    )]
    pub llama_server_ports: Option<PortMapping>,
}

const DEFAULT_ALLOWED_PROVIDERS: &[ProviderType] = &[
    ProviderType::Ollama,
    ProviderType::Vllm,
    ProviderType::LmStudio,
    ProviderType::LlamaServer,
];

impl ServeArgs {
    /// Get the port mapping for a specific provider type
    pub fn get_port_mapping(&self, provider_type: ProviderType) -> Option<&PortMapping> {
        match provider_type {
            ProviderType::Ollama => self.ollama_ports.as_ref(),
            ProviderType::Vllm => self.vllm_ports.as_ref(),
            ProviderType::LmStudio => self.lmstudio_ports.as_ref(),
            ProviderType::LlamaServer => self.llama_server_ports.as_ref(),
        }
    }

    /// Get all port mappings
    pub fn get_port_mappings(&self) -> HashMap<ProviderType, PortMapping> {
        let mut mappings = HashMap::new();

        for &provider_type in crate::ALL_PROVIDER_TYPES {
            if let Some(&mapping) = self.get_port_mapping(provider_type) {
                mappings.insert(provider_type, mapping);
            }
        }

        mappings
    }

    /// Get all allowed provider types
    /// Returns all providers if none specified
    pub fn get_allowed_providers(&self) -> Vec<ProviderType> {
        self.allowed_providers
            .clone()
            .unwrap_or_else(|| DEFAULT_ALLOWED_PROVIDERS.to_vec())
    }

    /// Check if a provider type is allowed
    pub fn is_provider_allowed(&self, provider_type: ProviderType) -> bool {
        match &self.allowed_providers {
            Some(providers) => providers.contains(&provider_type),
            None => true, // All providers allowed by default
        }
    }
}

#[derive(clap::Subcommand)]
pub enum DeviceCommands {
    /// Show your Device ID
    Show,
    /// Show list of allowed Device IDs
    List,
    /// Allow a given Device ID
    Allow { id: String },
    /// Disable a given Device ID
    Disable { id: String },
}

#[cfg(test)]
mod tests {
    use crate::ProviderType;

    use super::*;

    #[test]
    fn test_provider_type_from_str() {
        assert_eq!(
            "ollama".parse::<ProviderType>().unwrap(),
            ProviderType::Ollama
        );
        assert_eq!("vllm".parse::<ProviderType>().unwrap(), ProviderType::Vllm);
        assert_eq!(
            "lm-studio".parse::<ProviderType>().unwrap(),
            ProviderType::LmStudio
        );
        assert_eq!(
            "llama-server".parse::<ProviderType>().unwrap(),
            ProviderType::LlamaServer
        );

        // Only exact lowercase with hyphens allowed
        assert!("Ollama".parse::<ProviderType>().is_err());
        assert!("OLLAMA".parse::<ProviderType>().is_err());
        assert!("lmstudio".parse::<ProviderType>().is_err());
        assert!("llamaserver".parse::<ProviderType>().is_err());
        assert!("invalid".parse::<ProviderType>().is_err());
    }

    #[test]
    fn test_provider_type_display() {
        assert_eq!(ProviderType::Ollama.to_string(), "ollama");
        assert_eq!(ProviderType::Vllm.to_string(), "vllm");
        assert_eq!(ProviderType::LmStudio.to_string(), "lm-studio");
        assert_eq!(ProviderType::LlamaServer.to_string(), "llama-server");
    }

    #[test]
    fn test_port_mapping_two_ports() {
        let mapping = "11434:8888".parse::<PortMapping>().unwrap();
        assert_eq!(mapping.port1, Some(11434));
        assert_eq!(mapping.port2, Some(8888));
    }

    #[test]
    fn test_port_mapping_empty_first_port() {
        let mapping = ":8888".parse::<PortMapping>().unwrap();
        assert_eq!(mapping.port1, None);
        assert_eq!(mapping.port2, Some(8888));
    }

    #[test]
    fn test_port_mapping_empty_second_port() {
        let mapping = "11434:".parse::<PortMapping>().unwrap();
        assert_eq!(mapping.port1, Some(11434));
        assert_eq!(mapping.port2, None);
    }

    #[test]
    fn test_port_mapping_invalid_format() {
        assert!("11434:8888:9999".parse::<PortMapping>().is_err());
        assert!(":".parse::<PortMapping>().is_err());
        assert!("11434".parse::<PortMapping>().is_err());
        assert!("invalid".parse::<PortMapping>().is_err());
        assert!("11434:invalid".parse::<PortMapping>().is_err());
    }

    #[test]
    fn test_port_mapping_out_of_range() {
        assert!("70000".parse::<PortMapping>().is_err());
        assert!("11434:70000".parse::<PortMapping>().is_err());
    }
}
