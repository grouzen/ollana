use actix_cors::Cors;
use actix_web::{dev::ServerHandle, web, App, HttpRequest, HttpResponse, HttpServer};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use log::error;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{mpsc, oneshot::Sender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;

use crate::{constants, device::Device, HTTP_HEADER_OLLANA_DEVICE_ID};

pub const PROXY_DEFAULT_WORKERS_NUMBER: usize = 2;

#[async_trait]
pub trait ClientProxy: Send + Sync {
    async fn run_server(&mut self, tx: Sender<Box<dyn ClientProxy>>) -> anyhow::Result<()>;

    async fn stop(&self, graceful: bool);
}

#[derive(Clone)]
pub struct HttpClientProxy {
    client: reqwest::Client,
    host: String,
    port: u16,
    server_url: Url,
    handle: Option<ServerHandle>,
    device: Arc<dyn Device>,
}

pub struct HttpClientProxyBuilder {
    server_socket_addr: SocketAddr,
    device: Arc<dyn Device>,
    host: Option<String>,
    port: Option<u16>,
}

impl HttpClientProxyBuilder {
    pub fn new(server_socket_addr: SocketAddr, device: Arc<dyn Device>) -> Self {
        Self {
            server_socket_addr,
            device,
            host: None,
            port: None,
        }
    }

    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn build(self) -> anyhow::Result<HttpClientProxy> {
        let server_url = format!("https://{}", self.server_socket_addr);
        let server_url = Url::parse(&server_url)?;
        let client = reqwest::ClientBuilder::new()
            .use_rustls_tls()
            .danger_accept_invalid_certs(true)
            .build()?;

        Ok(HttpClientProxy {
            client,
            host: self
                .host
                .unwrap_or_else(|| constants::OLLANA_CLIENT_PROXY_DEFAULT_ADDRESS.to_string()),
            port: self
                .port
                .unwrap_or(constants::OLLANA_CLIENT_PROXY_DEFAULT_PORT),
            server_url,
            handle: None,
            device: self.device,
        })
    }
}

impl HttpClientProxy {
    pub fn new(server_socket_addr: SocketAddr, device: Arc<dyn Device>) -> anyhow::Result<Self> {
        HttpClientProxyBuilder::new(server_socket_addr, device).build()
    }

    pub fn builder(
        server_socket_addr: SocketAddr,
        device: Arc<dyn Device>,
    ) -> HttpClientProxyBuilder {
        HttpClientProxyBuilder::new(server_socket_addr, device)
    }

    async fn forward(
        req: HttpRequest,
        client: web::Data<reqwest::Client>,
        server_url: web::Data<Url>,
        device: web::Data<Arc<dyn Device>>,
        mut payload: web::Payload,
        method: actix_web::http::Method,
    ) -> Result<HttpResponse, actix_web::Error> {
        let (tx, rx) = mpsc::unbounded_channel();

        actix_web::rt::spawn(async move {
            while let Some(chunk) = payload.next().await {
                tx.send(chunk).unwrap();
            }
        });

        let mut server_uri = (**server_url).clone();
        server_uri.set_path(req.uri().path());
        server_uri.set_query(req.uri().query());

        let server_request = client
            .request(
                reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
                server_uri,
            )
            .header(HTTP_HEADER_OLLANA_DEVICE_ID, &device.get_id())
            .body(reqwest::Body::wrap_stream(UnboundedReceiverStream::new(rx)));

        let server_response = server_request
            .send()
            .await
            .map_err(actix_web::error::ErrorInternalServerError)?;

        let mut response = HttpResponse::build(
            actix_web::http::StatusCode::from_u16(server_response.status().as_u16()).unwrap(),
        );

        Ok(response.streaming(server_response.bytes_stream()))
    }
}

#[async_trait]
impl ClientProxy for HttpClientProxy {
    async fn run_server(&mut self, tx: Sender<Box<dyn ClientProxy>>) -> anyhow::Result<()> {
        let client = self.client.clone();
        let server_url = self.server_url.clone();
        let device = self.device.clone();

        let server = HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(client.clone()))
                .app_data(web::Data::new(server_url.clone()))
                .app_data(web::Data::new(device.clone()))
                .wrap(Cors::permissive())
                .default_service(web::to(Self::forward))
        })
        .bind((self.host.clone(), self.port))?
        .workers(PROXY_DEFAULT_WORKERS_NUMBER)
        .run();

        let handle = server.handle();
        self.handle = Some(handle);

        if tx.send(Box::new(self.clone())).is_err() {
            error!("Couldn't send an updated client proxy");
        }

        server.await.map_err(anyhow::Error::new)
    }

    async fn stop(&self, graceful: bool) {
        if let Some(handle) = &self.handle {
            handle.stop(graceful).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::header::ContentType, web, App, HttpServer};
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    /// Mock Device implementation for testing
    struct MockDevice {
        id: String,
        allowed_devices: Mutex<Vec<String>>,
    }

    impl MockDevice {
        fn new(id: &str) -> Self {
            Self {
                id: id.to_string(),
                allowed_devices: Mutex::new(vec![]),
            }
        }
    }

    impl Device for MockDevice {
        fn get_id(&self) -> String {
            self.id.clone()
        }

        fn allow(&self, id: String) -> anyhow::Result<bool> {
            let mut devices = self.allowed_devices.lock().unwrap();
            if !devices.contains(&id) {
                devices.push(id);
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn disable(&self, id: String) -> anyhow::Result<bool> {
            let mut devices = self.allowed_devices.lock().unwrap();
            if let Some(pos) = devices.iter().position(|x| *x == id) {
                devices.remove(pos);
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn is_allowed(&self, id: String) -> bool {
            let devices = self.allowed_devices.lock().unwrap();
            devices.contains(&id)
        }
    }

    /// Create rustls config for the test HTTPS server
    fn create_rustls_config() -> anyhow::Result<rustls::ServerConfig> {
        let subject_alt_names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
        let cert_key = generate_simple_self_signed(subject_alt_names)?;

        let cert_der = CertificateDer::from(cert_key.cert.der().to_vec());
        let key_der = PrivateKeyDer::try_from(cert_key.signing_key.serialize_der())
            .map_err(|e| anyhow::anyhow!("Failed to serialize key: {}", e))?;

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)?;

        Ok(config)
    }

    /// Helper function to start a mock LLM server that responds to LLM requests
    ///
    /// # Arguments
    /// * `port` - The port to bind the server to
    /// * `use_tls` - Whether to use TLS/HTTPS (true) or plain HTTP (false)
    async fn start_mock_llm_server(port: u16) -> anyhow::Result<ServerHandle> {
        let server = HttpServer::new(move || {
            App::new()
                .route("/api/version", web::get().to(mock_version_handler))
                .route("/api/generate", web::post().to(mock_generate_handler))
                .route("/api/chat", web::post().to(mock_chat_handler))
                .route("/api/tags", web::get().to(mock_tags_handler))
                .default_service(web::to(mock_default_handler))
        });
        let rustls_config = create_rustls_config()?;
        let server = server.bind_rustls_0_23(("127.0.0.1", port), rustls_config)?;
        let server = server.workers(1).run();

        let handle = server.handle();
        tokio::spawn(server);

        // Give the server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        Ok(handle)
    }

    async fn mock_version_handler() -> HttpResponse {
        HttpResponse::Ok()
            .content_type(ContentType::json())
            .body(r#"{"version":"0.1.0"}"#)
    }

    async fn mock_generate_handler(body: web::Bytes) -> HttpResponse {
        // Parse the body as JSON
        let body_json: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(json) => json,
            Err(_) => {
                return HttpResponse::BadRequest()
                    .content_type(ContentType::json())
                    .body(r#"{"error": "Invalid JSON"}"#);
            }
        };

        let prompt = body_json
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let response = serde_json::json!({
            "model": "test-model",
            "response": format!("Generated response for: {}", prompt),
            "done": true
        });

        HttpResponse::Ok()
            .content_type(ContentType::json())
            .body(response.to_string())
    }

    async fn mock_chat_handler(body: web::Bytes) -> HttpResponse {
        // Parse the body as JSON
        let body_json: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(json) => json,
            Err(_) => {
                return HttpResponse::BadRequest()
                    .content_type(ContentType::json())
                    .body(r#"{"error": "Invalid JSON"}"#);
            }
        };

        let _messages = body_json.get("messages").and_then(|v| v.as_array());
        let response = serde_json::json!({
            "model": "test-model",
            "message": {
                "role": "assistant",
                "content": "Test chat response"
            },
            "done": true
        });

        HttpResponse::Ok()
            .content_type(ContentType::json())
            .body(response.to_string())
    }

    async fn mock_tags_handler() -> HttpResponse {
        let response = serde_json::json!({
            "models": [
                {"name": "test-model:latest", "size": 1000}
            ]
        });

        HttpResponse::Ok()
            .content_type(ContentType::json())
            .body(response.to_string())
    }

    async fn mock_default_handler(req: HttpRequest, body: web::Bytes) -> HttpResponse {
        // Handle both with and without body
        let body_str = String::from_utf8_lossy(&body);
        let response = if body_str.is_empty() {
            format!(r#"{{"path": "{}"}}"#, req.uri().path())
        } else {
            format!(
                r#"{{"path": "{}", "body": {}}}"#,
                req.uri().path(),
                body_str
            )
        };

        HttpResponse::Ok()
            .content_type(ContentType::json())
            .body(response)
    }

    #[tokio::test]
    async fn test_http_client_proxy_run_server_starts_successfully() {
        let mock_server_port = 18080;
        let mock_server_handle = start_mock_llm_server(mock_server_port)
            .await
            .expect("Failed to start mock server");

        let device = Arc::new(MockDevice::new("test-device-1"));
        let server_addr: SocketAddr = format!("127.0.0.1:{}", mock_server_port).parse().unwrap();

        let proxy_port = 11433;
        let mut proxy = HttpClientProxy::builder(server_addr, device)
            .port(proxy_port)
            .build()
            .expect("Failed to create HttpClientProxy");

        let (tx, rx) = oneshot::channel();

        // Spawn the proxy server in the background
        tokio::spawn(async move { proxy.run_server(tx).await });

        // Wait for the proxy to be ready
        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // Stop the proxy gracefully
        updated_proxy.stop(true).await;

        // Stop the mock server
        mock_server_handle.stop(true).await;

        // Wait a bit for cleanup
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_client_proxy_forwards_requests() {
        let mock_server_port = 18081;
        let mock_server_handle = start_mock_llm_server(mock_server_port)
            .await
            .expect("Failed to start mock server");

        let device = Arc::new(MockDevice::new("test-device-2"));
        let server_addr: SocketAddr = format!("127.0.0.1:{}", mock_server_port).parse().unwrap();

        let proxy_port = 11435;
        let mut proxy = HttpClientProxy::builder(server_addr, device)
            .port(proxy_port)
            .build()
            .expect("Failed to create HttpClientProxy");

        let (tx, rx) = oneshot::channel();

        // Spawn the proxy server in the background
        tokio::spawn(async move { proxy.run_server(tx).await });

        // Wait for the proxy to be ready
        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // Give the proxy a moment to fully start
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Make a request through the proxy
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://127.0.0.1:{}/api/version", proxy_port))
            .send()
            .await
            .expect("Failed to send request through proxy");

        assert_eq!(response.status(), 200);

        let body = response.text().await.expect("Failed to read response body");
        assert!(body.contains("version"));

        // Stop the proxy
        updated_proxy.stop(true).await;
        mock_server_handle.stop(true).await;

        // Wait for cleanup
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_client_proxy_forwards_post_requests() {
        let mock_server_port = 18082;
        let mock_server_handle = start_mock_llm_server(mock_server_port)
            .await
            .expect("Failed to start mock server");

        let device = Arc::new(MockDevice::new("test-device-3"));
        let server_addr: SocketAddr = format!("127.0.0.1:{}", mock_server_port).parse().unwrap();

        let proxy_port = 11436;
        let mut proxy = HttpClientProxy::builder(server_addr, device)
            .port(proxy_port)
            .build()
            .expect("Failed to create HttpClientProxy");

        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Make a POST request through the proxy
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "prompt": "Hello, world!",
            "model": "test-model"
        });

        let response = client
            .post(format!("http://127.0.0.1:{}/api/generate", proxy_port))
            .json(&payload)
            .send()
            .await
            .expect("Failed to send POST request through proxy");

        assert_eq!(response.status(), 200);

        let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
        assert!(body.get("response").is_some());
        assert!(body
            .get("response")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("Generated response"));

        updated_proxy.stop(true).await;
        mock_server_handle.stop(true).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_client_proxy_stops_gracefully() {
        let mock_server_port = 18083;
        let mock_server_handle = start_mock_llm_server(mock_server_port)
            .await
            .expect("Failed to start mock server");

        let device = Arc::new(MockDevice::new("test-device-4"));
        let server_addr: SocketAddr = format!("127.0.0.1:{}", mock_server_port).parse().unwrap();

        let proxy_port = 11437;
        let mut proxy = HttpClientProxy::builder(server_addr, device)
            .port(proxy_port)
            .build()
            .expect("Failed to create HttpClientProxy");

        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Verify proxy is running by making a request
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://127.0.0.1:{}/api/version", proxy_port))
            .send()
            .await;

        assert!(response.is_ok());

        // Stop gracefully
        updated_proxy.stop(true).await;

        // Give it time to stop
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // Verify proxy is stopped by trying to connect (should fail)
        let response = client
            .get(format!("http://127.0.0.1:{}/api/version", proxy_port))
            .timeout(tokio::time::Duration::from_millis(500))
            .send()
            .await;

        assert!(response.is_err());

        mock_server_handle.stop(true).await;
    }

    #[tokio::test]
    async fn test_http_client_proxy_forwards_query_parameters() {
        let mock_server_port = 18084;
        let mock_server_handle = start_mock_llm_server(mock_server_port)
            .await
            .expect("Failed to start mock server");

        let device = Arc::new(MockDevice::new("test-device-5"));
        let server_addr: SocketAddr = format!("127.0.0.1:{}", mock_server_port).parse().unwrap();

        let proxy_port = 11438;
        let mut proxy = HttpClientProxy::builder(server_addr, device)
            .port(proxy_port)
            .build()
            .expect("Failed to create HttpClientProxy");

        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Make a request with query parameters
        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "http://127.0.0.1:{}/api/custom?param1=value1&param2=value2",
                proxy_port
            ))
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(response.status(), 200);

        updated_proxy.stop(true).await;
        mock_server_handle.stop(true).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_client_proxy_builder_pattern() {
        let mock_server_port = 18085;
        let mock_server_handle = start_mock_llm_server(mock_server_port)
            .await
            .expect("Failed to start mock server");

        let device = Arc::new(MockDevice::new("test-device-6"));
        let server_addr: SocketAddr = format!("127.0.0.1:{}", mock_server_port).parse().unwrap();

        // Test builder with custom host and port
        let proxy_port = 11439;
        let custom_host = "127.0.0.1";
        let mut proxy = HttpClientProxy::builder(server_addr, device)
            .host(custom_host)
            .port(proxy_port)
            .build()
            .expect("Failed to create HttpClientProxy");

        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Make a request to verify the proxy is running with custom settings
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{}:{}/api/version", custom_host, proxy_port))
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(response.status(), 200);

        updated_proxy.stop(true).await;
        mock_server_handle.stop(true).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
