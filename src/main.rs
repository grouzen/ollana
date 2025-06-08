use clap::Parser;
use env_logger::Env;
use ollana::{args::Args, serve_app::ServeApp};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    match Args::parse() {
        Args::Serve(args) => {
            let serve_app = ServeApp::from_args(args);

            serve_app.run()
        }
    }
}
