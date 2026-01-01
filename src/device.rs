use std::sync::Arc;

use crate::{certs::Certs, config::Config};

pub struct Device {
    pub id: String,
    pub config: Arc<Config>,
}

impl Device {
    /// Creates a new instance of the device.
    ///
    /// This constructor initializes the device by generating necessary certificates
    /// and loading allowed device IDs from the provided configuration.
    ///
    /// # Arguments
    ///
    /// * `certs` - A reference to the certificates required for initialization.
    /// * `config` - Configuration containing allowed devices.
    ///
    /// # Returns
    ///
    /// This function returns a new instance of the device wrapped in a `Result`.
    /// If any step fails, an error is returned with detailed information about what went wrong.
    ///
    pub fn new(certs: &Certs, config: Arc<Config>) -> anyhow::Result<Self> {
        certs.gen_device()?;

        let id = sha256::digest(certs.get_device_key_bytes()?);

        Ok(Self { id, config })
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
        let mut config = (*self.config).clone();
        let allowed_devices = config.allowed_devices.get_or_insert_with(Vec::new);
        if !allowed_devices.contains(&id) {
            allowed_devices.push(id);
            config.save()?;
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
        let mut config = (*self.config).clone();
        if let Some(allowed_devices) = &mut config.allowed_devices {
            if allowed_devices.contains(&id) {
                allowed_devices.retain(|x| x != &id);
                config.save()?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Checks whether a device is allowed.
    ///
    /// Returns `true` if the specified device ID is in the list of allowed devices; otherwise returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to check.
    ///
    pub fn is_allowed(&self, id: String) -> bool {
        self.config
            .allowed_devices
            .as_ref()
            .map(|devices| devices.contains(&id))
            .unwrap_or(false)
    }
}
