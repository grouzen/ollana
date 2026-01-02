use clap::Parser;
use env_logger::{Builder, Env};
use ollana::{
    args::{Args, DeviceCommands},
    certs::X509Certs,
    config::{Config, TomlConfig},
    device::{ConfigDevice, Device},
    get_local_dir,
    serve_app::ServeApp,
};
use std::{
    fs::OpenOptions,
    sync::{Arc, Mutex},
};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let certs = Arc::new(X509Certs::new()?);
    let local_dir = get_local_dir()?;
    let config: Arc<Mutex<TomlConfig>> = Arc::new(Mutex::new(TomlConfig::load(&local_dir)?));
    let device = Arc::new(ConfigDevice::new(certs.as_ref(), config.clone())?);

    match args {
        Args::Serve(args) => {
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
            println!("Device ID: {}", device.id);

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
