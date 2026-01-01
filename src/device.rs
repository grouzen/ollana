use std::sync::Arc;

use crate::{certs::Certs, config::Config};

/// Trait for device management operations.
/// This allows for different implementations of device management behavior.
pub trait Device: Send + Sync {
    /// Allows a device with the specified ID.
    ///
    /// If the device is not already allowed, it will be added to the list and the configuration saved.
    /// Returns `true` if the operation was successful and the device was added; otherwise returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to allow.
    ///
    fn allow(&self, id: String) -> anyhow::Result<bool>;

    /// Disables a device with the specified ID.
    ///
    /// If the device is currently allowed, it will be removed from the list and the configuration saved.
    /// Returns `true` if the operation was successful; otherwise. returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to disable.
    ///
    fn disable(&self, id: String) -> anyhow::Result<bool>;

    /// Checks whether a device is allowed.
    ///
    /// Returns `true` if the specified device ID is in the list of allowed devices; otherwise returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to check.
    ///
    fn is_allowed(&self, id: String) -> bool;
}

/// Config-backed implementation of Device.
pub struct ConfigDevice {
    pub id: String,
    pub config: Arc<Config>,
}

impl ConfigDevice {
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
}

impl Device for ConfigDevice {
    /// Allows a device with the specified ID.
    ///
    /// If the device is not already allowed, it will be added to the list and the configuration saved.
    /// Returns `true` if the operation was successful and the device was added; otherwise returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to allow.
    ///
    fn allow(&self, id: String) -> anyhow::Result<bool> {
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
    /// Returns `true` if the operation was successful; otherwise. returns `false`.
    ///
    /// # Arguments
    /// * `id`: The unique identifier of the device to disable.
    ///
    fn disable(&self, id: String) -> anyhow::Result<bool> {
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
    fn is_allowed(&self, id: String) -> bool {
        self.config
            .allowed_devices
            .as_ref()
            .map(|devices| devices.contains(&id))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock implementation of Device for testing.
    pub struct MockDevice {
        allowed_devices: Arc<std::sync::Mutex<Vec<String>>>,
        save_called: Arc<std::sync::Mutex<bool>>,
    }

    impl MockDevice {
        pub fn new(allowed_devices: Vec<String>) -> Self {
            Self {
                allowed_devices: Arc::new(std::sync::Mutex::new(allowed_devices)),
                save_called: Arc::new(std::sync::Mutex::new(false)),
            }
        }

        pub fn was_save_called(&self) -> bool {
            *self.save_called.lock().unwrap()
        }

        pub fn get_allowed_devices(&self) -> Vec<String> {
            self.allowed_devices.lock().unwrap().clone()
        }
    }

    impl Device for MockDevice {
        fn allow(&self, id: String) -> anyhow::Result<bool> {
            let mut devices = self.allowed_devices.lock().unwrap();
            if !devices.contains(&id) {
                devices.push(id);
                *self.save_called.lock().unwrap() = true;
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn disable(&self, id: String) -> anyhow::Result<bool> {
            let mut devices = self.allowed_devices.lock().unwrap();
            if devices.contains(&id) {
                devices.retain(|x| x != &id);
                *self.save_called.lock().unwrap() = true;
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn is_allowed(&self, id: String) -> bool {
            self.allowed_devices.lock().unwrap().contains(&id)
        }
    }

    #[test]
    fn test_allow_new_device() {
        let device = MockDevice::new(vec![]);
        let result = device.allow("device1".to_string());

        assert!(result.is_ok());
        assert!(result.unwrap());
        assert!(device.was_save_called());
        assert_eq!(device.get_allowed_devices(), vec!["device1"]);
    }

    #[test]
    fn test_allow_already_allowed_device() {
        let device = MockDevice::new(vec!["device1".to_string()]);
        let result = device.allow("device1".to_string());

        assert!(result.is_ok());
        assert!(!result.unwrap());
        assert!(!device.was_save_called());
        assert_eq!(device.get_allowed_devices(), vec!["device1"]);
    }

    #[test]
    fn test_allow_multiple_devices() {
        let device = MockDevice::new(vec!["device1".to_string()]);

        let result1 = device.allow("device2".to_string());
        assert!(result1.is_ok());
        assert!(result1.unwrap());

        let result2 = device.allow("device3".to_string());
        assert!(result2.is_ok());
        assert!(result2.unwrap());

        let allowed = device.get_allowed_devices();
        assert_eq!(allowed.len(), 3);
        assert!(allowed.contains(&"device1".to_string()));
        assert!(allowed.contains(&"device2".to_string()));
        assert!(allowed.contains(&"device3".to_string()));
    }

    #[test]
    fn test_disable_existing_device() {
        let device = MockDevice::new(vec!["device1".to_string(), "device2".to_string()]);
        let result = device.disable("device1".to_string());

        assert!(result.is_ok());
        assert!(result.unwrap());
        assert!(device.was_save_called());
        assert_eq!(device.get_allowed_devices(), vec!["device2"]);
    }

    #[test]
    fn test_disable_non_existing_device() {
        let device = MockDevice::new(vec!["device1".to_string()]);
        let result = device.disable("device2".to_string());

        assert!(result.is_ok());
        assert!(!result.unwrap());
        assert!(!device.was_save_called());
        assert_eq!(device.get_allowed_devices(), vec!["device1"]);
    }

    #[test]
    fn test_disable_from_empty_list() {
        let device = MockDevice::new(vec![]);
        let result = device.disable("device1".to_string());

        assert!(result.is_ok());
        assert!(!result.unwrap());
        assert!(!device.was_save_called());
        assert_eq!(device.get_allowed_devices(), Vec::<String>::new());
    }

    #[test]
    fn test_disable_all_devices() {
        let device = MockDevice::new(vec!["device1".to_string(), "device2".to_string()]);

        let result1 = device.disable("device1".to_string());
        assert!(result1.is_ok());
        assert!(result1.unwrap());

        let result2 = device.disable("device2".to_string());
        assert!(result2.is_ok());
        assert!(result2.unwrap());

        assert_eq!(device.get_allowed_devices(), Vec::<String>::new());
    }

    #[test]
    fn test_is_allowed_existing_device() {
        let device = MockDevice::new(vec!["device1".to_string(), "device2".to_string()]);

        assert!(device.is_allowed("device1".to_string()));
        assert!(device.is_allowed("device2".to_string()));
    }

    #[test]
    fn test_is_allowed_non_existing_device() {
        let device = MockDevice::new(vec!["device1".to_string()]);

        assert!(!device.is_allowed("device2".to_string()));
        assert!(!device.is_allowed("device3".to_string()));
    }

    #[test]
    fn test_is_allowed_empty_list() {
        let device = MockDevice::new(vec![]);

        assert!(!device.is_allowed("device1".to_string()));
    }

    #[test]
    fn test_is_allowed_after_allow() {
        let device = MockDevice::new(vec![]);

        assert!(!device.is_allowed("device1".to_string()));

        device.allow("device1".to_string()).unwrap();

        assert!(device.is_allowed("device1".to_string()));
    }

    #[test]
    fn test_is_allowed_after_disable() {
        let device = MockDevice::new(vec!["device1".to_string()]);

        assert!(device.is_allowed("device1".to_string()));

        device.disable("device1".to_string()).unwrap();

        assert!(!device.is_allowed("device1".to_string()));
    }
}
