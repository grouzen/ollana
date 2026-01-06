use crate::{PortMapping, ProviderType};
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
    pub allowed_providers: Option<Vec<ProviderType>>,

    /// Port mapping for Ollama provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_ports: Option<PortMapping>,

    /// Port mapping for vLLM provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vllm_ports: Option<PortMapping>,

    /// Port mapping for LM Studio provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lmstudio_ports: Option<PortMapping>,

    /// Port mapping for llama.cpp server provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llama_server_ports: Option<PortMapping>,

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
        // Check for port conflicts
        let mut used_ports: HashMap<u16, Vec<String>> = HashMap::new();

        for (provider_type, mapping) in self.get_port_mappings() {
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
        let ollama_ports = config.ollama_ports.unwrap();
        assert_eq!(ollama_ports.port1, Some(11434));
        assert_eq!(ollama_ports.port2, Some(8888));
        let vllm_ports = config.vllm_ports.unwrap();
        assert_eq!(vllm_ports.port1, Some(8000));
        assert_eq!(vllm_ports.port2, Some(8001));
    }

    #[test]
    fn test_save_valid_config() {
        let temp_dir = TempDir::new().unwrap();
        let config = TomlConfig {
            allowed_providers: Some(vec![ProviderType::Ollama, ProviderType::Vllm]),
            ollama_ports: Some(PortMapping {
                port1: Some(11434),
                port2: Some(8888),
            }),
            vllm_ports: Some(PortMapping {
                port1: Some(8000),
                port2: Some(8001),
            }),
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
        assert!(result.unwrap_err().to_string().contains("unknown variant"));
    }

    #[test]
    fn test_save_config_with_port_conflict() {
        let temp_dir = TempDir::new().unwrap();
        let config = TomlConfig {
            ollama_ports: Some(PortMapping {
                port1: Some(11434),
                port2: Some(8888),
            }),
            vllm_ports: Some(PortMapping {
                port1: Some(8000),
                port2: Some(8888),
            }), // Conflict on port 8888
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
            allowed_providers: Some(vec![ProviderType::Ollama]),
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
