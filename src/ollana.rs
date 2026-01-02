use std::net::SocketAddr;

use async_trait::async_trait;
use http::StatusCode;
use log::debug;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::HTTP_HEADER_OLLANA_DEVICE_ID;

#[derive(Serialize, Deserialize)]
pub struct AuthorizationResponse {
    pub device_id: String,
}

impl AuthorizationResponse {
    pub fn new(device_id: String) -> Self {
        Self { device_id }
    }
}

/// Trait for Ollana authorization operations.
/// This allows for different implementations of authorization behavior.
#[async_trait]
pub trait Ollana: Send + Sync {
    /// Checks if a device is authorized to access Ollana API.
    ///
    /// This function sends an HTTP POST request to the `/ollana/api/authorize`
    /// endpoint with the specified device ID. If the response status code is
    /// `UNAUTHORIZED`, it logs the failure and returns `None`. Otherwise, it parses
    /// the JSON response as an `AuthorizationResponse` and returns it wrapped in
    /// `Some`.
    ///
    /// # Arguments
    ///
    /// * `device_id`: A `String` representing the unique identifier for a device.
    ///
    /// # Returns
    ///
    /// * An `anyhow::Result<Option<AuthorizationResponse>>` indicating success or failure,
    ///   with an optional authorization response if the request is successful and authorized.
    ///
    async fn check_authorization(
        &self,
        device_id: String,
    ) -> anyhow::Result<Option<AuthorizationResponse>>;
}

/// HTTP-backed implementation of Ollana trait.
pub struct HttpOllana {
    client: reqwest::Client,
    url: Url,
}

impl HttpOllana {
    pub fn new(socket_addr: SocketAddr) -> anyhow::Result<Self> {
        let url = format!("https://{}", socket_addr);
        let url = Url::parse(&url).unwrap();
        let client = reqwest::ClientBuilder::new()
            .use_rustls_tls()
            .danger_accept_invalid_certs(true)
            .build()?;

        Ok(Self { client, url })
    }
}

#[async_trait]
impl Ollana for HttpOllana {
    async fn check_authorization(
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
    use rustls::ServerConfig;
    use std::net::SocketAddr;
    use tokio;

    /// Helper function to create a simple HTTPS test server with self-signed certificate
    /// that returns a successful authorization response
    async fn start_mock_auth_server() -> (
        SocketAddr,
        tokio::task::JoinHandle<Result<(), std::io::Error>>,
    ) {
        // Generate self-signed certificate for testing
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = cert.cert.der().to_vec();
        let key_der = cert.signing_key.serialize_der();

        // Create rustls config
        let cert_chain = vec![rustls::pki_types::CertificateDer::from(cert_der)];
        let key = rustls::pki_types::PrivateKeyDer::try_from(key_der).unwrap();

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .unwrap();

        let server = HttpServer::new(|| {
            App::new().route(
                "/ollana/api/authorize",
                web::post().to(|req: HttpRequest| async move {
                    let device_id = req
                        .headers()
                        .get(HTTP_HEADER_OLLANA_DEVICE_ID)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown");

                    // Simulate unauthorized for specific device
                    if device_id == "unauthorized-device" {
                        return HttpResponse::Unauthorized().body("Device not authorized");
                    }

                    // Return successful authorization
                    HttpResponse::Ok().json(serde_json::json!({
                        "device_id": device_id
                    }))
                }),
            )
        })
        .bind_rustls_0_23(("127.0.0.1", 0), config)
        .expect("Failed to bind test server");

        let addr = server.addrs()[0];
        let handle = tokio::spawn(server.run());

        // Give the server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        (addr, handle)
    }

    #[tokio::test]
    async fn test_check_authorization_success() {
        let (addr, _handle) = start_mock_auth_server().await;
        let ollana = HttpOllana::new(addr).unwrap();

        let result = ollana
            .check_authorization("test-device-123".to_string())
            .await;

        assert!(result.is_ok(), "Authorization should succeed");
        let response = result.unwrap();
        assert!(response.is_some(), "Should return authorization response");
        assert_eq!(
            response.unwrap().device_id,
            "test-device-123",
            "Device ID should match"
        );
    }

    #[tokio::test]
    async fn test_check_authorization_unauthorized() {
        let (addr, _handle) = start_mock_auth_server().await;
        let ollana = HttpOllana::new(addr).unwrap();

        let result = ollana
            .check_authorization("unauthorized-device".to_string())
            .await;

        assert!(result.is_ok(), "Request should succeed");
        let response = result.unwrap();
        assert!(
            response.is_none(),
            "Should return None for unauthorized device"
        );
    }

    #[tokio::test]
    async fn test_check_authorization_different_devices() {
        let (addr, _handle) = start_mock_auth_server().await;
        let ollana = HttpOllana::new(addr).unwrap();

        // Test multiple different device IDs
        for device_id in &["device-1", "device-2", "device-abc-xyz"] {
            let result = ollana
                .check_authorization(device_id.to_string())
                .await
                .unwrap();

            assert!(
                result.is_some(),
                "Device {} should be authorized",
                device_id
            );
            assert_eq!(
                result.unwrap().device_id,
                *device_id,
                "Device ID should match for {}",
                device_id
            );
        }
    }
}
