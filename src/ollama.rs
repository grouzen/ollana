use std::net::ToSocketAddrs;

use serde::Deserialize;
use url::Url;

use crate::{constants, error::OllanaError};

pub struct Ollama {
    pub host: String,
    pub port: u16,
}

impl Default for Ollama {
    fn default() -> Self {
        Self {
            host: constants::OLLAMA_DEFAULT_ADDRESS.to_string(),
            port: constants::OLLAMA_DEFAULT_PORT,
        }
    }
}

#[derive(Deserialize)]
pub struct VersionResponse {
    #[allow(dead_code)]
    version: String,
}

impl Ollama {
    pub async fn get_version() -> crate::error::Result<VersionResponse> {
        let ollama_socket_addr = (
            constants::OLLAMA_DEFAULT_ADDRESS.to_string(),
            constants::OLLAMA_DEFAULT_PORT,
        )
            .to_socket_addrs()
            .map_err(OllanaError::from)?
            .next()
            .expect("server proxy address is invalid");
        let ollama_version_url = format!("http://{ollama_socket_addr}/api/version");
        let ollama_version_url = Url::parse(&ollama_version_url).unwrap();

        reqwest::get(ollama_version_url)
            .await
            .map_err(OllanaError::from)?
            .json::<VersionResponse>()
            .await
            .map_err(OllanaError::from)
    }
}
