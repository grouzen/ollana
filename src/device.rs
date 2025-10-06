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
    /// Creates a new instance of the device.
    ///
    /// This constructor initializes the device by setting up its configuration,
    /// generating necessary certificates, and ensuring it is properly initialized.
    /// It performs the following steps:
    /// - Retrieves the local directory path.
    /// - Initializes the configuration file if not already done.
    /// - Generates the device certificate using provided certs.
    /// - Computes a unique identifier for the device based on its key bytes.
    /// - Loads allowed device IDs from the configuration directory.
    ///
    /// # Arguments
    ///
    /// * `certs` - A reference to the certificates required for initialization.
    ///
    /// # Returns
    ///
    /// This function returns a new instance of the device wrapped in a `Result`.
    /// If any step fails, an error is returned with detailed information about what went wrong.
    ///
    pub fn new(certs: &Certs) -> anyhow::Result<Self> {
        let dir = get_local_dir()?;

        Self::init_config(&dir)?;
        certs.gen_device()?;

        let id = sha256::digest(certs.get_device_key_bytes()?);
        let allowed = Self::load_allowed_device_ids(&dir)?;

        Ok(Self { id, allowed, dir })
    }

    /// Allows a device with the specified ID.
    ///
    /// If the device is not already allowed, it will be added to the list and the configuration saved.
    /// Returns `true` if the operation was successful and the device was added; otherwise returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to allow.
    ///
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

    /// Disables a device with the specified ID.
    ///
    /// If the device is currently allowed, it will be removed from the list and the configuration saved.
    /// Returns `true` if the operation was successful; otherwise returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to disable.
    ///
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

    /// Checks whether a device is allowed.
    ///
    /// Returns `true` if the specified device ID is in the list of allowed devices; otherwise returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to check.
    ///
    pub fn is_allowed(&self, id: String) -> bool {
        self.allowed.contains(&id)
    }

    // Loads a vector containing all allowed device IDs from the configuration file.
    ///
    /// # Arguments
    /// * `dir`: A reference to the directory where the configuration file resides.
    ///
    fn load_allowed_device_ids(dir: &Path) -> anyhow::Result<Vec<String>> {
        let config = Self::load_config(dir)?;

        Ok(config.allowed)
    }

    /// Initializes the device configuration if it does not already exist in the specified directory.
    ///
    /// If a configuration file is missing, this method creates one using default values and saves it to disk.
    ///
    /// # Arguments
    /// * `dir`: A reference to the directory where the configuration should be initialized.
    ///
    fn init_config(dir: &Path) -> anyhow::Result<()> {
        if !dir.join(DEVICE_CONFIG_TOML).as_path().exists() {
            let config = DeviceConfig::default();

            Self::save_config(dir, &config)?;
        }

        Ok(())
    }

    /// Loads the device configuration from a TOML file.
    ///
    /// # Arguments
    /// * `dir`: A reference to the directory where the configuration file is located.
    ///
    fn load_config(dir: &Path) -> anyhow::Result<DeviceConfig> {
        let toml_str = std::fs::read_to_string(dir.join(DEVICE_CONFIG_TOML))?;

        toml::from_str(&toml_str).map_err(anyhow::Error::from)
    }

    /// Saves the device configuration to a TOML file.
    ///
    /// # Arguments
    /// * `dir`: A reference to the directory where the configuration should be saved.
    /// * `config`: The device configuration object to serialize and save.
    ///
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
