use clap::Parser;
use env_logger::{Builder, Env};
use ollana::{args::Args, serve_app::ServeApp};
use std::fs::OpenOptions;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

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

            let serve_app = ServeApp::from_args(args);

            serve_app.run()
        }
    }
}
