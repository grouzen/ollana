use std::path::PathBuf;

pub mod args;
pub mod certs;
pub mod constants;
pub mod device;
pub mod discovery;
pub mod manager;
pub mod ollama;
pub mod proxy;
pub mod serve_app;

pub enum Mode {
    Client,
    Server,
}

fn get_local_dir() -> anyhow::Result<PathBuf> {
    dirs::data_local_dir()
        .map(|p| p.join("ollana"))
        .ok_or(anyhow::Error::msg(
            "Couldn't determine data local directory",
        ))
}
