use std::net::ToSocketAddrs;

use serde::Deserialize;
use url::Url;

use crate::{constants, error::OllanaError};

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
    pub fn try_new(host: String, port: u16) -> crate::error::Result<Ollama> {
        let socket_addr = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| OllanaError::Other("Ollama address is invalid".to_string()))?;
        let url = format!("http://{socket_addr}");
        let url = Url::parse(&url).map_err(OllanaError::from)?;

        Ok(Ollama {
            url: url,
            ..Default::default()
        })
    }

    pub async fn get_version(&self) -> crate::error::Result<VersionResponse> {
        let mut uri = self.url.clone();
        uri.set_path("api/version");

        self.client
            .get(uri)
            .send()
            .await
            .map_err(OllanaError::from)?
            .json::<VersionResponse>()
            .await
            .map_err(OllanaError::from)
    }
}
