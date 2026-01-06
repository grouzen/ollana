use clap::Parser;
use env_logger::{Builder, Env};
use ollana::{
    args::{Args, DeviceCommands, ServeArgs},
    certs::X509Certs,
    config::{Config, TomlConfig},
    device::{ConfigDevice, Device},
    get_local_dir,
    serve_app::ServeApp,
    PortMapping, ProviderType, ALL_PROVIDER_TYPES,
};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    sync::{Arc, Mutex},
};

fn merge_port_mappings(
    args_ports: HashMap<ProviderType, PortMapping>,
    config_ports: HashMap<ProviderType, PortMapping>,
) -> HashMap<ProviderType, PortMapping> {
    let mut mappings = HashMap::new();

    for &provider_type in ALL_PROVIDER_TYPES {
        if let Some(&mapping) = args_ports.get(&provider_type) {
            mappings.insert(provider_type, mapping);
        } else if let Some(&mapping) = config_ports.get(&provider_type) {
            mappings.insert(provider_type, mapping);
        }
    }

    mappings
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let certs = Arc::new(X509Certs::new()?);
    let local_dir = get_local_dir()?;
    let config: Arc<Mutex<TomlConfig>> = Arc::new(Mutex::new(TomlConfig::load(&local_dir)?));
    let device = Arc::new(ConfigDevice::new(certs.as_ref(), config.clone())?);

    match args {
        Args::Serve(args) => {
            // Merge config file with CLI args (CLI takes precedence)
            let merged_port_mappings = merge_port_mappings(
                args.get_port_mappings(),
                config.lock().unwrap().get_port_mappings(),
            );
            let args = ServeArgs {
                ollama_ports: merged_port_mappings.get(&ProviderType::Ollama).cloned(),
                vllm_ports: merged_port_mappings.get(&ProviderType::Vllm).cloned(),
                lmstudio_ports: merged_port_mappings.get(&ProviderType::LmStudio).cloned(),
                llama_server_ports: merged_port_mappings
                    .get(&ProviderType::LlamaServer)
                    .cloned(),
                ..args
            };
            // Configure logging based on whether a log file was specified
            let mut builder = Builder::from_env(Env::default().default_filter_or("info"));

            if let Some(log_file_path) = &args.log_file {
                // Create the log file if it doesn't exist
                let log_file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log_file_path)
                    .map_err(|e| anyhow::anyhow!("Failed to create/open log file: {}", e))?;

                builder
                    .target(env_logger::Target::Pipe(Box::new(log_file)))
                    .init();
            } else {
                // Use default logging to stdout
                builder.init();
            }

            let serve_app = ServeApp::new(args, certs, device)?;

            serve_app.run()
        }
        Args::Device(DeviceCommands::Show) => {
            println!("Device ID: {}", device.get_id());

            Ok(())
        }
        Args::Device(DeviceCommands::List) => {
            println!("Allowed Device IDs:");
            let allowed_devices = config.lock().unwrap().get_allowed_devices();
            for id in allowed_devices.iter().flatten() {
                println!("{}", id);
            }

            Ok(())
        }
        Args::Device(DeviceCommands::Allow { id }) => {
            let is_allowed = device.allow(id.clone())?;

            if is_allowed {
                println!("Added Device ID: {}", id);
            } else {
                println!("The given Device ID has been allowed already")
            }

            Ok(())
        }
        Args::Device(DeviceCommands::Disable { id }) => {
            let is_disabled = device.disable(id.clone())?;

            if is_disabled {
                println!("Removed Device ID: {}", id);
            } else {
                println!("The given Device ID has not beed allowed");
            }

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_port_mappings_args_take_precedence() {
        let mut args_ports = HashMap::new();
        args_ports.insert(
            ProviderType::Ollama,
            PortMapping {
                port1: Some(11111),
                port2: Some(22222),
            },
        );

        let mut config_ports = HashMap::new();
        config_ports.insert(
            ProviderType::Ollama,
            PortMapping {
                port1: Some(33333),
                port2: Some(44444),
            },
        );

        let result = merge_port_mappings(args_ports, config_ports);

        let ollama = result.get(&ProviderType::Ollama).unwrap();
        assert_eq!(ollama.port1, Some(11111));
        assert_eq!(ollama.port2, Some(22222));
    }

    #[test]
    fn test_merge_port_mappings_falls_back_to_config() {
        let args_ports = HashMap::new();

        let mut config_ports = HashMap::new();
        config_ports.insert(
            ProviderType::Vllm,
            PortMapping {
                port1: Some(8000),
                port2: Some(8001),
            },
        );

        let result = merge_port_mappings(args_ports, config_ports);

        let vllm = result.get(&ProviderType::Vllm).unwrap();
        assert_eq!(vllm.port1, Some(8000));
        assert_eq!(vllm.port2, Some(8001));
    }

    #[test]
    fn test_merge_port_mappings_empty_inputs() {
        let args_ports = HashMap::new();
        let config_ports = HashMap::new();

        let result = merge_port_mappings(args_ports, config_ports);

        assert!(result.is_empty());
    }

    #[test]
    fn test_merge_port_mappings_multiple_providers() {
        let mut args_ports = HashMap::new();
        args_ports.insert(
            ProviderType::Ollama,
            PortMapping {
                port1: Some(11434),
                port2: Some(11435),
            },
        );
        args_ports.insert(
            ProviderType::LmStudio,
            PortMapping {
                port1: Some(1234),
                port2: Some(1235),
            },
        );

        let mut config_ports = HashMap::new();
        config_ports.insert(
            ProviderType::Ollama,
            PortMapping {
                port1: Some(55555),
                port2: Some(55556),
            },
        );
        config_ports.insert(
            ProviderType::Vllm,
            PortMapping {
                port1: Some(8000),
                port2: Some(8001),
            },
        );
        config_ports.insert(
            ProviderType::LlamaServer,
            PortMapping {
                port1: Some(8080),
                port2: Some(8081),
            },
        );

        let result = merge_port_mappings(args_ports, config_ports);

        // Ollama from args (takes precedence)
        assert_eq!(
            result.get(&ProviderType::Ollama).unwrap().port1,
            Some(11434)
        );
        // LmStudio from args
        assert_eq!(
            result.get(&ProviderType::LmStudio).unwrap().port1,
            Some(1234)
        );
        // Vllm from config (no args override)
        assert_eq!(result.get(&ProviderType::Vllm).unwrap().port1, Some(8000));
        // LlamaServer from config (no args override)
        assert_eq!(
            result.get(&ProviderType::LlamaServer).unwrap().port1,
            Some(8080)
        );
    }
}
