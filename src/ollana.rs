use std::net::SocketAddr;

use http::StatusCode;
use log::debug;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::HTTP_HEADER_OLLANA_DEVICE_ID;

pub struct Ollana {
    client: reqwest::Client,
    url: Url,
}

#[derive(Serialize, Deserialize)]
pub struct AuthorizationResponse {
    pub device_id: String,
}

impl AuthorizationResponse {
    pub fn new(device_id: String) -> Self {
        Self { device_id }
    }
}

impl Ollana {
    pub fn new(socket_addr: SocketAddr) -> anyhow::Result<Self> {
        let url = format!("https://{}", socket_addr);
        let url = Url::parse(&url).unwrap();
        let client = reqwest::ClientBuilder::new()
            .use_rustls_tls()
            .danger_accept_invalid_certs(true)
            .build()?;

        Ok(Self { client, url })
    }

    pub async fn check_authorization(
        &self,
        device_id: String,
    ) -> anyhow::Result<Option<AuthorizationResponse>> {
        let mut uri = self.url.clone();
        uri.set_path("ollana/api/authorize");

        match self
            .client
            .post(uri)
            .header(HTTP_HEADER_OLLANA_DEVICE_ID, &device_id)
            .send()
            .await?
        {
            response if response.status() == StatusCode::UNAUTHORIZED => {
                let message = response.text().await?;

                debug!(
                    "Ollana authorization failed for device_id {}: {}",
                    device_id, message
                );

                Ok(None)
            }
            response => response
                .json::<AuthorizationResponse>()
                .await
                .map(Some)
                .map_err(anyhow::Error::new),
        }
    }
}
