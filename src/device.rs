use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{certs::Certs, get_local_dir};

const DEVICE_CONFIG_TOML: &str = "device_allowed.toml";

pub struct Device {
    pub id: String,
    pub allowed: Vec<String>,
    dir: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct DeviceConfig {
    allowed: Vec<String>,
}

impl Device {
    pub fn new(certs: &Certs) -> anyhow::Result<Self> {
        let dir = get_local_dir()?;

        Self::init_config(&dir)?;
        certs.gen_device()?;

        let id = sha256::digest(certs.get_device_key_bytes()?);
        let allowed = Self::load_allowed_device_ids(&dir)?;

        Ok(Self { id, allowed, dir })
    }

    pub fn allow(&self, id: String) -> anyhow::Result<bool> {
        let mut config = Self::load_config(&self.dir)?;

        if !config.allowed.contains(&id) {
            config.allowed.push(id);
            Self::save_config(&self.dir, &config)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn disable(&self, id: String) -> anyhow::Result<bool> {
        let mut config = Self::load_config(&self.dir)?;

        if config.allowed.contains(&id) {
            config.allowed.retain(|x| x != &id);
            Self::save_config(&self.dir, &config)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn is_allowed(&self, id: String) -> bool {
        self.allowed.contains(&id)
    }

    fn load_allowed_device_ids(dir: &Path) -> anyhow::Result<Vec<String>> {
        let config = Self::load_config(dir)?;

        Ok(config.allowed)
    }

    fn init_config(dir: &Path) -> anyhow::Result<()> {
        if !dir.join(DEVICE_CONFIG_TOML).as_path().exists() {
            let config = DeviceConfig::default();

            Self::save_config(dir, &config)?;
        }

        Ok(())
    }

    fn load_config(dir: &Path) -> anyhow::Result<DeviceConfig> {
        let toml_str = std::fs::read_to_string(dir.join(DEVICE_CONFIG_TOML))?;

        toml::from_str(&toml_str).map_err(anyhow::Error::from)
    }

    fn save_config(dir: &Path, config: &DeviceConfig) -> anyhow::Result<()> {
        let mut config_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(dir.join(DEVICE_CONFIG_TOML))
            .map_err(|e| anyhow::anyhow!("Failed to create/open a device config file: {}", e))?;
        let toml_str = toml::to_string_pretty(&config)?;

        config_file.write_all(toml_str.as_bytes())?;

        Ok(())
    }
}
