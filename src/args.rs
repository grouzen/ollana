use clap::Parser;

#[derive(Parser)]
#[command(name = "ollana")]
#[command(bin_name = "ollana")]
#[command(version, about)]
pub enum Args {
    Serve(ServeArgs),
}

#[derive(clap::Args)]
pub struct ServeArgs {
    #[arg(short = 'd', long, default_value_t = true, help = "Run in daemon mode")]
    pub daemon: bool,
}
