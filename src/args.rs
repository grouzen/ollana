use clap::Parser;

#[derive(Parser)]
#[command(name = "ollana")]
#[command(bin_name = "ollana")]
#[command(version, about)]
pub enum Args {
    /// Run the ollana server
    Serve(ServeArgs),
    #[clap(subcommand)]
    /// Manage devices
    Device(DeviceCommands),
}

#[derive(clap::Args)]
pub struct ServeArgs {
    #[arg(
        short = 'd',
        long,
        default_value_t = false,
        help = "Run in daemon mode"
    )]
    pub daemon: bool,
    #[arg(
        long = "pid",
        value_name = "PID_FILE",
        help = "PID file path (only valid when --daemon is used)",
        required = false,
        requires = "daemon"
    )]
    pub pid_file: Option<std::path::PathBuf>,
    #[arg(
        long = "log-file",
        value_name = "LOG_FILE",
        help = "Log file path",
        required = false
    )]
    pub log_file: Option<std::path::PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum DeviceCommands {
    /// Show your Device ID
    Show,
    /// Show list of allowed Device IDs
    List,
    /// Allow a given Device ID
    Allow { id: String },
    /// Disable a given Device ID
    Disable { id: String },
}
