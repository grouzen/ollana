use std::{fs::OpenOptions, io::Write, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{certs::Certs, get_local_dir};

const DEVICE_CONFIG_TOML: &str = "device_allowed.toml";

pub struct Device {
    pub id: String,
    dir: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct DeviceConfig {
    allowed: Vec<String>,
}

impl Device {
    pub fn new(certs: &Certs) -> anyhow::Result<Self> {
        certs.gen_device()?;

        let id = sha256::digest(certs.get_device_key_bytes()?);
        let dir = get_local_dir()?;

        Ok(Self { id, dir })
    }

    pub fn allow_device_id(&self, id: String) -> anyhow::Result<bool> {
        let mut config = self.load_config()?;

        if !config.allowed.contains(&id) {
            config.allowed.push(id);
            self.save_config(&config)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn disable_device_id(&self, id: String) -> anyhow::Result<bool> {
        let mut config = self.load_config()?;

        if config.allowed.contains(&id) {
            config.allowed.retain(|x| x != &id);
            self.save_config(&config)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn list_allowed_device_ids(&self) -> anyhow::Result<Vec<String>> {
        let config = self.load_config()?;

        Ok(config.allowed)
    }

    pub fn init_config(&self) -> anyhow::Result<()> {
        if !self.dir.join(DEVICE_CONFIG_TOML).as_path().exists() {
            let config = DeviceConfig::default();

            self.save_config(&config)?;
        }

        Ok(())
    }

    fn load_config(&self) -> anyhow::Result<DeviceConfig> {
        let toml_str = std::fs::read_to_string(self.dir.join(DEVICE_CONFIG_TOML))?;

        toml::from_str(&toml_str).map_err(anyhow::Error::from)
    }

    fn save_config(&self, config: &DeviceConfig) -> anyhow::Result<()> {
        let mut config_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(self.dir.join(DEVICE_CONFIG_TOML))
            .map_err(|e| anyhow::anyhow!("Failed to create/open a device config file: {}", e))?;
        let toml_str = toml::to_string_pretty(&config)?;

        config_file.write_all(toml_str.as_bytes())?;

        Ok(())
    }
}
