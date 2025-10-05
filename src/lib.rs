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
