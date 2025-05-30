use std::net::{SocketAddr, ToSocketAddrs};

use serde::Deserialize;
use url::Url;

use crate::constants;

pub struct Ollama {
    client: reqwest::Client,
    url: Url,
}

impl Default for Ollama {
    fn default() -> Self {
        let url = format!(
            "http://{}:{}",
            constants::OLLAMA_DEFAULT_ADDRESS,
            constants::OLLAMA_DEFAULT_PORT
        );
        let url = Url::parse(&url).unwrap();

        Self {
            client: reqwest::Client::default(),
            url: url,
        }
    }
}

#[derive(Deserialize)]
pub struct VersionResponse {
    #[allow(dead_code)]
    version: String,
}

impl Ollama {
    pub fn try_new(host: String, port: u16) -> anyhow::Result<Self> {
        let socket_addr = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::Error::msg("Ollama address is invalid".to_string()))?;

        Self::from_socket_addr(socket_addr)
    }

    pub fn from_socket_addr(socket_addr: SocketAddr) -> anyhow::Result<Self> {
        let url = format!("http://{socket_addr}");
        let url = Url::parse(&url)?;

        Ok(Ollama {
            url: url,
            ..Default::default()
        })
    }

    pub async fn get_version(&self) -> anyhow::Result<VersionResponse> {
        let mut uri = self.url.clone();
        uri.set_path("api/version");

        self.client
            .get(uri)
            .send()
            .await?
            .json::<VersionResponse>()
            .await
            .map_err(anyhow::Error::new)
    }
}
