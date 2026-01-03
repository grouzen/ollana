use crate::args::{PortMapping, ProviderType};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Name of the configuration file
const CONFIG_FILE_NAME: &str = "config.toml";

/// Trait for configuration management operations.
/// Allows different implementations of configuration storage and validation.
pub trait Config: Send + Sync {
    /// Load configuration from a directory
    fn load(dir: &Path) -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Save configuration
    fn save(&self) -> anyhow::Result<()>;

    /// Get allowed devices
    fn get_allowed_devices(&self) -> Option<Vec<String>>;

    /// Set allowed devices
    fn set_allowed_devices(&mut self, devices: Option<Vec<String>>);
}

/// TOML-backed implementation of Config trait
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TomlConfig {
    /// Comma-separated list of allowed provider types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_providers: Option<Vec<String>>,

    /// Port mapping for Ollama provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_ports: Option<String>,

    /// Port mapping for vLLM provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vllm_ports: Option<String>,

    /// Port mapping for LM Studio provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lmstudio_ports: Option<String>,

    /// Port mapping for llama.cpp server provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llama_server_ports: Option<String>,

    /// List of allowed device IDs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_devices: Option<Vec<String>>,

    /// Directory path where the config file is located (not serialized)
    #[serde(skip)]
    pub dir: PathBuf,
}

impl TomlConfig {
    /// Validate the configuration
    ///
    /// Checks for:
    /// - Valid provider types
    /// - Valid port mapping formats
    /// - Port conflicts (same port used for multiple providers)
    fn validate(&self) -> anyhow::Result<()> {
        // Validate allowed providers
        self.parse_allowed_providers()?;

        // Validate and collect port mappings
        let port_mappings = self.parse_port_mappings()?;

        // Check for port conflicts
        let mut used_ports: HashMap<u16, Vec<String>> = HashMap::new();

        for (provider_type, mapping) in &port_mappings {
            let provider_name = provider_type.to_string();

            // Check port1 (if present)
            if let Some(port) = mapping.port1 {
                used_ports
                    .entry(port)
                    .or_default()
                    .push(format!("{} (port1)", provider_name));
            }

            // Check port2 (if present)
            if let Some(port) = mapping.port2 {
                used_ports
                    .entry(port)
                    .or_default()
                    .push(format!("{} (port2)", provider_name));
            }
        }

        // Report port conflicts
        for (port, users) in used_ports {
            if users.len() > 1 {
                return Err(anyhow::anyhow!(
                    "Port conflict: port {} is used by multiple providers: {}",
                    port,
                    users.join(", ")
                ));
            }
        }

        Ok(())
    }

    /// Parse allowed providers from string list
    fn parse_allowed_providers(&self) -> anyhow::Result<Option<Vec<ProviderType>>> {
        match &self.allowed_providers {
            Some(providers) => {
                let mut parsed = Vec::new();
                for provider_str in providers {
                    let provider = provider_str
                        .parse::<ProviderType>()
                        .map_err(|e| anyhow::anyhow!("Invalid provider in config file: {}", e))?;
                    parsed.push(provider);
                }
                Ok(Some(parsed))
            }
            None => Ok(None),
        }
    }

    /// Parse a port mapping string
    fn parse_port_mapping(port_str: &str, provider: &str) -> anyhow::Result<PortMapping> {
        port_str
            .parse::<PortMapping>()
            .map_err(|e| anyhow::anyhow!("Invalid {} port mapping in config file: {}", provider, e))
    }

    /// Parse all port mappings from config
    fn parse_port_mappings(&self) -> anyhow::Result<HashMap<ProviderType, PortMapping>> {
        let mut mappings = HashMap::new();

        if let Some(ref ollama_ports) = self.ollama_ports {
            mappings.insert(
                ProviderType::Ollama,
                Self::parse_port_mapping(ollama_ports, "Ollama")?,
            );
        }

        if let Some(ref vllm_ports) = self.vllm_ports {
            mappings.insert(
                ProviderType::Vllm,
                Self::parse_port_mapping(vllm_ports, "vLLM")?,
            );
        }

        if let Some(ref lmstudio_ports) = self.lmstudio_ports {
            mappings.insert(
                ProviderType::LmStudio,
                Self::parse_port_mapping(lmstudio_ports, "LM Studio")?,
            );
        }

        if let Some(ref llama_server_ports) = self.llama_server_ports {
            mappings.insert(
                ProviderType::LlamaServer,
                Self::parse_port_mapping(llama_server_ports, "llama.cpp server")?,
            );
        }

        Ok(mappings)
    }
}

impl Default for TomlConfig {
    fn default() -> Self {
        Self {
            allowed_providers: None,
            ollama_ports: None,
            vllm_ports: None,
            lmstudio_ports: None,
            llama_server_ports: None,
            allowed_devices: None,
            dir: PathBuf::new(),
        }
    }
}

impl Config for TomlConfig {
    /// Load configuration from the config file in the given directory
    ///
    /// If the config file doesn't exist, returns a default (empty) configuration.
    /// If the config file exists but cannot be parsed, returns an error.
    fn load(dir: &Path) -> anyhow::Result<Self> {
        let config_path = dir.join(CONFIG_FILE_NAME);

        if !config_path.exists() {
            log::debug!(
                "Config file not found at {}, using defaults",
                config_path.display()
            );
            return Ok(Self {
                dir: dir.to_path_buf(),
                ..Default::default()
            });
        }

        let toml_str = std::fs::read_to_string(&config_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to read config file at {}: {}",
                config_path.display(),
                e
            )
        })?;

        let mut config: Self = toml::from_str(&toml_str).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse config file at {}: {}",
                config_path.display(),
                e
            )
        })?;

        config.dir = dir.to_path_buf();

        // Validate the loaded configuration
        config.validate()?;

        Ok(config)
    }

    /// Save configuration to the config file
    ///
    /// Creates the config file if it doesn't exist.
    fn save(&self) -> anyhow::Result<()> {
        // Validate before saving
        self.validate()?;

        let config_path = self.dir.join(CONFIG_FILE_NAME);
        let mut config_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&config_path)
            .map_err(|e| anyhow::anyhow!("Failed to create/open config file: {}", e))?;

        let toml_str = toml::to_string_pretty(self)?;
        config_file.write_all(toml_str.as_bytes())?;

        Ok(())
    }

    /// Get allowed devices
    fn get_allowed_devices(&self) -> Option<Vec<String>> {
        self.allowed_devices.clone()
    }

    /// Set allowed devices
    fn set_allowed_devices(&mut self, devices: Option<Vec<String>>) {
        self.allowed_devices = devices;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, path::PathBuf};
    use tempfile::TempDir;

    fn create_config_file(dir: &Path, content: &str) -> anyhow::Result<PathBuf> {
        let config_path = dir.join(CONFIG_FILE_NAME);
        let mut file = std::fs::File::create(&config_path)?;
        file.write_all(content.as_bytes())?;
        Ok(config_path)
    }

    #[test]
    fn test_load_missing_config_returns_default() {
        let temp_dir = TempDir::new().unwrap();
        let config = TomlConfig::load(temp_dir.path()).unwrap();

        assert!(config.allowed_devices.is_none());
        assert_eq!(config.dir, temp_dir.path());
    }

    #[test]
    fn test_load_valid_config() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"
allowed_providers = ["ollama", "vllm"]
ollama_ports = "11434:8888"
vllm_ports = "8000:8001"
"#;
        create_config_file(temp_dir.path(), content).unwrap();

        let config = TomlConfig::load(temp_dir.path()).unwrap();

        // Access fields directly for validation tests
        assert_eq!(config.allowed_providers.as_ref().unwrap().len(), 2);
        assert_eq!(config.ollama_ports.as_deref(), Some("11434:8888"));
        assert_eq!(config.vllm_ports.as_deref(), Some("8000:8001"));
    }

    #[test]
    fn test_save_valid_config() {
        let temp_dir = TempDir::new().unwrap();
        let config = TomlConfig {
            allowed_providers: Some(vec!["ollama".to_string(), "vllm".to_string()]),
            ollama_ports: Some("11434:8888".to_string()),
            vllm_ports: Some("8000:8001".to_string()),
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // Should succeed because config is valid
        assert!(config.save().is_ok());
    }

    #[test]
    fn test_load_config_with_invalid_provider() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"
allowed_providers = ["invalid_provider"]
"#;
        create_config_file(temp_dir.path(), content).unwrap();

        // Should fail during load due to invalid provider
        let result = TomlConfig::load(temp_dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid provider"));
    }

    #[test]
    fn test_save_config_with_port_conflict() {
        let temp_dir = TempDir::new().unwrap();
        let config = TomlConfig {
            ollama_ports: Some("11434:8888".to_string()),
            vllm_ports: Some("8000:8888".to_string()), // Conflict on port 8888
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // Should fail during save due to port conflict
        let result = config.save();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Port conflict"));
    }

    #[test]
    fn test_load_config_with_allowed_devices() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"
allowed_devices = [
    "device_id_1",
    "device_id_2",
    "device_id_3"
]
"#;
        create_config_file(temp_dir.path(), content).unwrap();

        let config = TomlConfig::load(temp_dir.path()).unwrap();

        assert!(config.allowed_devices.is_some());
        let devices = config.allowed_devices.as_ref().unwrap();
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0], "device_id_1");
        assert_eq!(devices[1], "device_id_2");
        assert_eq!(devices[2], "device_id_3");
    }

    #[test]
    fn test_load_config_with_empty_allowed_devices() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"
allowed_devices = []
"#;
        create_config_file(temp_dir.path(), content).unwrap();

        let config = TomlConfig::load(temp_dir.path()).unwrap();

        assert!(config.allowed_devices.is_some());
        assert_eq!(config.allowed_devices.as_ref().unwrap().len(), 0);
    }

    #[test]
    fn test_load_config_without_allowed_devices() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"
allowed_providers = ["ollama"]
"#;
        create_config_file(temp_dir.path(), content).unwrap();

        let config = TomlConfig::load(temp_dir.path()).unwrap();

        assert!(config.allowed_devices.is_none());
    }

    #[test]
    fn test_save_config_with_allowed_devices() {
        let temp_dir = TempDir::new().unwrap();
        let config = TomlConfig {
            allowed_devices: Some(vec!["device_1".to_string(), "device_2".to_string()]),
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        config.save().unwrap();

        // Reload and verify
        let loaded_config = TomlConfig::load(temp_dir.path()).unwrap();
        assert!(loaded_config.allowed_devices.is_some());
        let devices = loaded_config.allowed_devices.as_ref().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0], "device_1");
        assert_eq!(devices[1], "device_2");
    }

    #[test]
    fn test_allowed_devices_not_serialized_when_none() {
        let temp_dir = TempDir::new().unwrap();
        let config = TomlConfig {
            allowed_providers: Some(vec!["ollama".to_string()]),
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        config.save().unwrap();

        // Read the file content directly
        let config_path = temp_dir.path().join(CONFIG_FILE_NAME);
        let content = std::fs::read_to_string(&config_path).unwrap();

        // Verify allowed_devices is not in the file when None
        assert!(!content.contains("allowed_devices"));
        assert!(content.contains("allowed_providers"));
    }
}
