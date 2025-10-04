use std::net::SocketAddr;

use serde::Deserialize;
use url::Url;

use crate::constants;

#[derive(Clone)]
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
        let client = reqwest::Client::default();

        Self { client, url }
    }
}

#[derive(Deserialize)]
pub struct VersionResponse {
    #[allow(dead_code)]
    version: String,
}

impl Ollama {
    pub fn from_socket_addr(socket_addr: SocketAddr, secure: bool) -> anyhow::Result<Self> {
        let url_schema = if secure { "https" } else { "http" };
        let url = format!("{}://{}", url_schema, socket_addr);
        let url = Url::parse(&url)?;
        let client = reqwest::ClientBuilder::new()
            .use_rustls_tls()
            .danger_accept_invalid_certs(true)
            .build()?;

        Ok(Ollama { client, url })
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
