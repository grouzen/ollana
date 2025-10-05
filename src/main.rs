use clap::Parser;
use env_logger::{Builder, Env};
use ollana::{
    args::{Args, DeviceCommands},
    certs::Certs,
    device::Device,
    serve_app::ServeApp,
};
use std::{fs::OpenOptions, sync::Arc};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let certs = Arc::new(Certs::new()?);
    let device = Device::new(&certs)?;

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

            let serve_app = ServeApp::from_args(args, certs)?;

            serve_app.run()
        }
        Args::Device(DeviceCommands::Show) => {
            println!("Device ID: {}", device.id);

            Ok(())
        }
        Args::Device(DeviceCommands::Allow { .. }) => Ok(()),
    }
}
