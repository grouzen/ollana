use std::path::PathBuf;

pub mod args;
pub mod certs;
pub mod client_manager;
pub mod config;
pub mod constants;
pub mod device;
pub mod discovery;
pub mod ollana;
pub mod provider;
pub mod proxy;
pub mod serve_app;

// Include generated protobuf code
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/ollana.discovery.rs"));
}

pub const HTTP_HEADER_OLLANA_DEVICE_ID: &str = "X-Ollana-Device-Id";

pub enum Mode {
    Client,
    Server,
}

/// Returns the path to the local data directory used by Ollana.
///
/// This method attempts to determine the location of the application's local data directory using
/// the `dirs` crate. If successful, it returns a `PathBuf` pointing to a subdirectory named
/// "ollana" within this directory.
///
/// # Returns
/// A `Result<PathBuf>` indicating success or failure:
/// - Ok(PathBuf): The path to the local data directory for Ollana.
/// - Err(anyhow::Error): An error if the local data directory cannot be determined.
///
/// # Errors
/// This function can return an `anyhow::Error` if it fails to determine the data local directory.
///
pub fn get_local_dir() -> anyhow::Result<PathBuf> {
    dirs::data_local_dir()
        .map(|p| p.join("ollana"))
        .ok_or(anyhow::Error::msg(
            "Couldn't determine data local directory",
        ))
}
