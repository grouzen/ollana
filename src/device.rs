use std::sync::{Arc, Mutex};

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
    pub config: Arc<Mutex<dyn Config>>,
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
    pub fn new(certs: &dyn Certs, config: Arc<Mutex<dyn Config>>) -> anyhow::Result<Self> {
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
        let mut config = self.config.lock().unwrap();
        let mut allowed_devices = config.get_allowed_devices().unwrap_or_default();
        if !allowed_devices.contains(&id) {
            allowed_devices.push(id);
            config.set_allowed_devices(Some(allowed_devices));
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
        let mut config = self.config.lock().unwrap();
        if let Some(mut allowed_devices) = config.get_allowed_devices() {
            if allowed_devices.contains(&id) {
                allowed_devices.retain(|x| x != &id);
                config.set_allowed_devices(Some(allowed_devices));
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
            .lock()
            .unwrap()
            .get_allowed_devices()
            .as_ref()
            .map(|devices| devices.contains(&id))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Mock implementation of Config for testing.
    struct MockConfig {
        allowed_devices: Vec<String>,
        save_called: AtomicBool,
    }

    impl MockConfig {
        fn new(allowed_devices: Vec<String>) -> Self {
            Self {
                allowed_devices,
                save_called: AtomicBool::new(false),
            }
        }
    }

    impl Config for MockConfig {
        fn load(_dir: &std::path::Path) -> anyhow::Result<Self>
        where
            Self: Sized,
        {
            Ok(Self::new(vec![]))
        }

        fn save(&self) -> anyhow::Result<()> {
            self.save_called.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn get_allowed_devices(&self) -> Option<Vec<String>> {
            if self.allowed_devices.is_empty() {
                None
            } else {
                Some(self.allowed_devices.clone())
            }
        }

        fn set_allowed_devices(&mut self, devices: Option<Vec<String>>) {
            self.allowed_devices = devices.unwrap_or_default();
        }
    }

    fn create_test_device(allowed_devices: Vec<String>) -> (ConfigDevice, Arc<Mutex<MockConfig>>) {
        let config = Arc::new(Mutex::new(MockConfig::new(allowed_devices)));
        let device = ConfigDevice {
            id: "test_device".to_string(),
            config: config.clone(),
        };
        (device, config)
    }

    #[test]
    fn test_allow_new_device() {
        let (device, config) = create_test_device(vec![]);
        let result = device.allow("device1".to_string());

        assert!(result.is_ok());
        assert!(result.unwrap());

        let locked_config = config.lock().unwrap();
        assert!(locked_config.save_called.load(Ordering::SeqCst));
        assert_eq!(
            locked_config.get_allowed_devices().unwrap(),
            vec!["device1"]
        );
    }

    #[test]
    fn test_allow_already_allowed_device() {
        let (device, config) = create_test_device(vec!["device1".to_string()]);
        let result = device.allow("device1".to_string());

        assert!(result.is_ok());
        assert!(!result.unwrap());

        let locked_config = config.lock().unwrap();
        assert!(!locked_config.save_called.load(Ordering::SeqCst));
        assert_eq!(
            locked_config.get_allowed_devices().unwrap(),
            vec!["device1"]
        );
    }

    #[test]
    fn test_allow_multiple_devices() {
        let (device, config) = create_test_device(vec!["device1".to_string()]);

        let result1 = device.allow("device2".to_string());
        assert!(result1.is_ok());
        assert!(result1.unwrap());

        let result2 = device.allow("device3".to_string());
        assert!(result2.is_ok());
        assert!(result2.unwrap());

        let locked_config = config.lock().unwrap();
        let allowed = locked_config.get_allowed_devices().unwrap();
        assert_eq!(allowed.len(), 3);
        assert!(allowed.contains(&"device1".to_string()));
        assert!(allowed.contains(&"device2".to_string()));
        assert!(allowed.contains(&"device3".to_string()));
    }

    #[test]
    fn test_disable_existing_device() {
        let (device, config) =
            create_test_device(vec!["device1".to_string(), "device2".to_string()]);
        let result = device.disable("device1".to_string());

        assert!(result.is_ok());
        assert!(result.unwrap());

        let locked_config = config.lock().unwrap();
        assert!(locked_config.save_called.load(Ordering::SeqCst));
        assert_eq!(
            locked_config.get_allowed_devices().unwrap(),
            vec!["device2"]
        );
    }

    #[test]
    fn test_disable_non_existing_device() {
        let (device, config) = create_test_device(vec!["device1".to_string()]);
        let result = device.disable("device2".to_string());

        assert!(result.is_ok());
        assert!(!result.unwrap());

        let locked_config = config.lock().unwrap();
        assert!(!locked_config.save_called.load(Ordering::SeqCst));
        assert_eq!(
            locked_config.get_allowed_devices().unwrap(),
            vec!["device1"]
        );
    }

    #[test]
    fn test_disable_from_empty_list() {
        let (device, config) = create_test_device(vec![]);
        let result = device.disable("device1".to_string());

        assert!(result.is_ok());
        assert!(!result.unwrap());

        let locked_config = config.lock().unwrap();
        assert!(!locked_config.save_called.load(Ordering::SeqCst));
        assert!(locked_config.get_allowed_devices().is_none());
    }

    #[test]
    fn test_disable_all_devices() {
        let (device, config) =
            create_test_device(vec!["device1".to_string(), "device2".to_string()]);

        let result1 = device.disable("device1".to_string());
        assert!(result1.is_ok());
        assert!(result1.unwrap());

        let result2 = device.disable("device2".to_string());
        assert!(result2.is_ok());
        assert!(result2.unwrap());

        let locked_config = config.lock().unwrap();
        assert!(locked_config.get_allowed_devices().is_none());
    }

    #[test]
    fn test_is_allowed_existing_device() {
        let (device, _) = create_test_device(vec!["device1".to_string(), "device2".to_string()]);

        assert!(device.is_allowed("device1".to_string()));
        assert!(device.is_allowed("device2".to_string()));
    }

    #[test]
    fn test_is_allowed_non_existing_device() {
        let (device, _) = create_test_device(vec!["device1".to_string()]);

        assert!(!device.is_allowed("device2".to_string()));
        assert!(!device.is_allowed("device3".to_string()));
    }

    #[test]
    fn test_is_allowed_empty_list() {
        let (device, _) = create_test_device(vec![]);

        assert!(!device.is_allowed("device1".to_string()));
    }

    #[test]
    fn test_is_allowed_after_allow() {
        let (device, _) = create_test_device(vec![]);

        assert!(!device.is_allowed("device1".to_string()));

        device.allow("device1".to_string()).unwrap();

        assert!(device.is_allowed("device1".to_string()));
    }

    #[test]
    fn test_is_allowed_after_disable() {
        let (device, _) = create_test_device(vec!["device1".to_string()]);

        assert!(device.is_allowed("device1".to_string()));

        device.disable("device1".to_string()).unwrap();

        assert!(!device.is_allowed("device1".to_string()));
    }
}
