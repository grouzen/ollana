use actix_web::{
    dev::ServerHandle, error, http::header::ContentType, web, App, Error, HttpRequest,
    HttpResponse, HttpServer,
};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use log::{debug, error};
use std::{fs::File, io::BufReader, sync::Arc};
use tokio::sync::{mpsc, oneshot::Sender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;

use crate::{
    certs::Certs, constants, device::Device, ollana::AuthorizationResponse,
    HTTP_HEADER_OLLANA_DEVICE_ID,
};

pub const PROXY_DEFAULT_WORKERS_NUMBER: usize = 2;

#[async_trait]
pub trait ServerProxy: Send + Sync {
    async fn run_server(&mut self, tx: Sender<Box<dyn ServerProxy>>) -> anyhow::Result<()>;

    async fn stop(&self, graceful: bool);
}

pub struct HttpServerProxyBuilder {
    device: Arc<dyn Device>,
    certs: Arc<dyn Certs>,
    host: Option<String>,
    port: Option<u16>,
    provider_url: Option<Url>,
}

impl HttpServerProxyBuilder {
    pub fn new(device: Arc<dyn Device>, certs: Arc<dyn Certs>) -> Self {
        Self {
            device,
            certs,
            host: None,
            port: None,
            provider_url: None,
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

    // TODO: provider_url must be required and passed from server_manager
    pub fn provider_url(mut self, url: Url) -> Self {
        self.provider_url = Some(url);
        self
    }

    pub fn build(self) -> HttpServerProxy {
        let provider_url = self.provider_url.unwrap_or_else(|| {
            let url_str = format!(
                "http://{}:{}",
                constants::OLLAMA_DEFAULT_ADDRESS,
                constants::OLLAMA_DEFAULT_PORT
            );
            Url::parse(&url_str).unwrap()
        });

        HttpServerProxy {
            client: reqwest::Client::default(),
            host: self
                .host
                .unwrap_or_else(|| constants::OLLANA_SERVER_PROXY_DEFAULT_ADDRESS.to_string()),
            port: self
                .port
                .unwrap_or(constants::OLLANA_SERVER_PROXY_DEFAULT_PORT),
            provider_url,
            device: self.device,
            handle: None,
            certs: self.certs,
        }
    }
}

#[derive(Clone)]
pub struct HttpServerProxy {
    client: reqwest::Client,
    host: String,
    port: u16,
    provider_url: Url,
    device: Arc<dyn Device>,
    handle: Option<ServerHandle>,
    certs: Arc<dyn Certs>,
}

impl HttpServerProxy {
    pub fn builder(device: Arc<dyn Device>, certs: Arc<dyn Certs>) -> HttpServerProxyBuilder {
        HttpServerProxyBuilder::new(device, certs)
    }

    fn rustls_config(cert_file: File, key_file: File) -> anyhow::Result<rustls::ServerConfig> {
        let cert_reader = &mut BufReader::new(cert_file);
        let key_reader = &mut BufReader::new(key_file);

        let tls_certs = rustls_pemfile::certs(cert_reader).collect::<Result<Vec<_>, _>>()?;
        let mut tls_keys =
            rustls_pemfile::pkcs8_private_keys(key_reader).collect::<Result<Vec<_>, _>>()?;

        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                tls_certs,
                rustls::pki_types::PrivateKeyDer::Pkcs8(tls_keys.remove(0)),
            )
            .map_err(anyhow::Error::from)
    }

    fn is_authorized(req: HttpRequest, device: Arc<dyn Device>) -> bool {
        let device_id = req
            .headers()
            .get(HTTP_HEADER_OLLANA_DEVICE_ID)
            .and_then(|v| v.to_str().ok().map(String::from));

        debug!(
            "Authorization decision: uri_path = {}, device_id = {:?}",
            req.uri().path(),
            device_id
        );

        device_id.is_some_and(|id| device.is_allowed(id))
    }

    async fn authorize(
        req: HttpRequest,
        device: web::Data<Arc<dyn Device>>,
    ) -> Result<HttpResponse, actix_web::Error> {
        let device = (**device).clone();

        if Self::is_authorized(req.clone(), device.clone()) {
            let payload = AuthorizationResponse::new(device.get_id());
            let body = serde_json::to_string(&payload)?;

            Ok(HttpResponse::Ok()
                .content_type(ContentType::json())
                .body(body))
        } else {
            Ok(HttpResponse::Unauthorized()
                .content_type("text/plan")
                .body("Device is not authorized"))
        }
    }

    async fn forward(
        req: HttpRequest,
        client: web::Data<reqwest::Client>,
        provider_url: web::Data<Url>,
        device: web::Data<Arc<dyn Device>>,
        mut payload: web::Payload,
        method: actix_web::http::Method,
    ) -> Result<HttpResponse, Error> {
        let is_ignored_uri_path = req.uri().path() == "/api/version";

        if is_ignored_uri_path || Self::is_authorized(req.clone(), (**device).clone()) {
            let (tx, rx) = mpsc::unbounded_channel();

            actix_web::rt::spawn(async move {
                while let Some(chunk) = payload.next().await {
                    tx.send(chunk).unwrap();
                }
            });

            let mut provider_uri = (**provider_url).clone();
            provider_uri.set_path(req.uri().path());
            provider_uri.set_query(req.uri().query());

            let provider_request = client
                .request(
                    reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
                    provider_uri,
                )
                .body(reqwest::Body::wrap_stream(UnboundedReceiverStream::new(rx)));

            let provider_response = provider_request
                .send()
                .await
                .map_err(error::ErrorInternalServerError)?;

            let mut response = HttpResponse::build(
                actix_web::http::StatusCode::from_u16(provider_response.status().as_u16()).unwrap(),
            );

            Ok(response.streaming(provider_response.bytes_stream()))
        } else {
            Ok(HttpResponse::Unauthorized()
                .content_type("text/plan")
                .body("Device is not authorized"))
        }
    }
}

#[async_trait]
impl ServerProxy for HttpServerProxy {
    async fn run_server(&mut self, tx: Sender<Box<dyn ServerProxy>>) -> anyhow::Result<()> {
        let client = self.client.clone();
        let provider_url = self.provider_url.clone();
        let device = self.device.clone();

        let (cert_file, key_file) = self.certs.get_http_server_files()?;
        let rustls_config = Self::rustls_config(cert_file, key_file)?;

        let server = HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(client.clone()))
                .app_data(web::Data::new(provider_url.clone()))
                .app_data(web::Data::new(device.clone()))
                .service(
                    web::scope("/ollana/api").route("/authorize", web::post().to(Self::authorize)),
                )
                .default_service(web::to(Self::forward))
        })
        .bind_rustls_0_23((self.host.clone(), self.port), rustls_config)?
        .workers(PROXY_DEFAULT_WORKERS_NUMBER)
        .run();

        let handle = server.handle();
        self.handle = Some(handle);

        if tx.send(Box::new(self.clone())).is_err() {
            error!("Couldn't send an updated server proxy");
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
    use actix_web::{web, App, HttpServer};
    use rcgen::generate_simple_self_signed;
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

    /// Mock Certs implementation for testing
    struct MockCerts;

    impl Certs for MockCerts {
        fn gen_device(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_device_key_bytes(&self) -> anyhow::Result<Vec<u8>> {
            Ok(vec![])
        }

        fn gen_http_server(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_http_server_files(&self) -> anyhow::Result<(File, File)> {
            // Generate self-signed cert for testing
            let subject_alt_names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
            let cert_key = generate_simple_self_signed(subject_alt_names)?;

            // Create temporary files
            let cert_file = tempfile::NamedTempFile::new()?;
            let key_file = tempfile::NamedTempFile::new()?;

            // Write certificate
            std::io::Write::write_all(&mut cert_file.as_file(), cert_key.cert.pem().as_bytes())?;

            // Write key
            std::io::Write::write_all(
                &mut key_file.as_file(),
                cert_key.signing_key.serialize_pem().as_bytes(),
            )?;

            // Reopen files for reading
            let cert_path = cert_file.path().to_path_buf();
            let key_path = key_file.path().to_path_buf();

            // Keep temp files alive
            std::mem::forget(cert_file);
            std::mem::forget(key_file);

            Ok((File::open(cert_path)?, File::open(key_path)?))
        }
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

        let server = server.bind(("127.0.0.1", port))?;
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
    async fn test_http_server_proxy_run_server_starts_successfully() {
        // Start mock LLM server (simulates Ollama)
        let mock_llm_port = 19000;
        let mock_llm_handle = start_mock_llm_server(mock_llm_port)
            .await
            .expect("Failed to start mock LLM server");

        let device = Arc::new(MockDevice::new("server-test-device-1"));
        let provider_url = Url::parse(&format!("http://127.0.0.1:{mock_llm_port}")).unwrap();

        let proxy_port = 12000;
        let mut proxy = HttpServerProxy::builder(device, Arc::new(MockCerts))
            .port(proxy_port)
            .provider_url(provider_url)
            .build();

        let (tx, rx) = oneshot::channel();

        // Spawn the proxy server in the background
        let proxy_handle = tokio::spawn(async move { proxy.run_server(tx).await });

        // Wait for proxy to be ready
        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // Verify the proxy is running by making a request to /api/version (which doesn't require auth)
        let client = reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let response = client
            .get(format!("https://127.0.0.1:{proxy_port}/api/version"))
            .send()
            .await;

        assert!(response.is_ok(), "Proxy should be running and responding");
        assert_eq!(response.unwrap().status(), 200);

        // Cleanup
        updated_proxy.stop(true).await;
        proxy_handle.abort();
        mock_llm_handle.stop(true).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_server_proxy_forwards_authorized_requests() {
        // Start mock LLM server
        let mock_llm_port = 19001;
        let mock_llm_handle = start_mock_llm_server(mock_llm_port)
            .await
            .expect("Failed to start mock LLM server");

        let device = Arc::new(MockDevice::new("server-test-device-2"));
        let device_id = device.get_id();
        device.allow(device_id.clone()).unwrap();

        let provider_url = Url::parse(&format!("http://127.0.0.1:{mock_llm_port}")).unwrap();

        let proxy_port = 12001;
        let mut proxy = HttpServerProxy::builder(device, Arc::new(MockCerts))
            .port(proxy_port)
            .provider_url(provider_url)
            .build();

        let (tx, rx) = oneshot::channel();

        let proxy_handle = tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // Make an authorized request
        let client = reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let payload = serde_json::json!({
            "prompt": "Test prompt",
            "model": "test-model"
        });

        let response = client
            .post(format!("https://127.0.0.1:{proxy_port}/api/generate"))
            .header(HTTP_HEADER_OLLANA_DEVICE_ID, device_id)
            .json(&payload)
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(response.status(), 200);

        let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
        assert!(body.get("response").is_some());
        assert!(body
            .get("response")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("Generated response"));

        // Cleanup
        updated_proxy.stop(true).await;
        proxy_handle.abort();
        mock_llm_handle.stop(true).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_server_proxy_rejects_unauthorized_requests() {
        // Start mock LLM server
        let mock_llm_port = 19002;
        let mock_llm_handle = start_mock_llm_server(mock_llm_port)
            .await
            .expect("Failed to start mock LLM server");

        let device = Arc::new(MockDevice::new("server-test-device-3"));
        // Note: NOT allowing any device IDs

        let provider_url = Url::parse(&format!("http://127.0.0.1:{mock_llm_port}")).unwrap();

        let proxy_port = 12002;
        let mut proxy = HttpServerProxy::builder(device, Arc::new(MockCerts))
            .port(proxy_port)
            .provider_url(provider_url)
            .build();

        let (tx, rx) = oneshot::channel();

        let proxy_handle = tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // Make an unauthorized request (with wrong device ID)
        let client = reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let payload = serde_json::json!({
            "prompt": "Test prompt",
            "model": "test-model"
        });

        let response = client
            .post(format!("https://127.0.0.1:{proxy_port}/api/generate"))
            .header(HTTP_HEADER_OLLANA_DEVICE_ID, "unauthorized-device")
            .json(&payload)
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(response.status(), 401);

        let body = response.text().await.expect("Failed to read body");
        assert!(body.contains("not authorized"));

        // Cleanup
        updated_proxy.stop(true).await;
        proxy_handle.abort();
        mock_llm_handle.stop(true).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_server_proxy_authorize_endpoint() {
        // Start mock LLM server
        let mock_llm_port = 19003;
        let mock_llm_handle = start_mock_llm_server(mock_llm_port)
            .await
            .expect("Failed to start mock LLM server");

        let device = Arc::new(MockDevice::new("server-test-device-4"));
        let device_id = device.get_id();
        device.allow(device_id.clone()).unwrap();

        let provider_url = Url::parse(&format!("http://127.0.0.1:{mock_llm_port}")).unwrap();

        let proxy_port = 12003;
        let mut proxy = HttpServerProxy::builder(device, Arc::new(MockCerts))
            .port(proxy_port)
            .provider_url(provider_url)
            .build();

        let (tx, rx) = oneshot::channel();

        let proxy_handle = tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // Test authorized request
        let client = reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let response = client
            .post(format!(
                "https://127.0.0.1:{proxy_port}/ollana/api/authorize"
            ))
            .header(HTTP_HEADER_OLLANA_DEVICE_ID, device_id.clone())
            .send()
            .await
            .expect("Failed to send authorize request");

        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
        assert_eq!(body.get("device_id").unwrap().as_str().unwrap(), device_id);

        // Test unauthorized request
        let response = client
            .post(format!(
                "https://127.0.0.1:{proxy_port}/ollana/api/authorize"
            ))
            .header(HTTP_HEADER_OLLANA_DEVICE_ID, "wrong-device-id")
            .send()
            .await
            .expect("Failed to send authorize request");

        assert_eq!(response.status(), 401);

        // Cleanup
        updated_proxy.stop(true).await;
        proxy_handle.abort();
        mock_llm_handle.stop(true).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_server_proxy_version_endpoint_no_auth_required() {
        // Start mock LLM server
        let mock_llm_port = 19004;
        let mock_llm_handle = start_mock_llm_server(mock_llm_port)
            .await
            .expect("Failed to start mock LLM server");

        let device = Arc::new(MockDevice::new("server-test-device-5"));
        // Note: NOT allowing any device IDs

        let provider_url = Url::parse(&format!("http://127.0.0.1:{mock_llm_port}")).unwrap();

        let proxy_port = 12004;
        let mut proxy = HttpServerProxy::builder(device, Arc::new(MockCerts))
            .port(proxy_port)
            .provider_url(provider_url)
            .build();

        let (tx, rx) = oneshot::channel();

        let proxy_handle = tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // /api/version should work without authorization
        let client = reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let response = client
            .get(format!("https://127.0.0.1:{}/api/version", proxy_port))
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(response.status(), 200);
        let body = response.text().await.expect("Failed to read body");
        assert!(body.contains("version"));

        // Cleanup
        updated_proxy.stop(true).await;
        proxy_handle.abort();
        mock_llm_handle.stop(true).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_http_server_proxy_builder_pattern() {
        // Start mock LLM server
        let mock_llm_port = 19005;
        let mock_llm_handle = start_mock_llm_server(mock_llm_port)
            .await
            .expect("Failed to start mock LLM server");

        let device = Arc::new(MockDevice::new("server-test-device-6"));
        let provider_url = Url::parse(&format!("http://127.0.0.1:{mock_llm_port}")).unwrap();

        // Test builder with custom settings
        let proxy_port = 12005;
        let custom_host = "127.0.0.1";
        let mut proxy = HttpServerProxy::builder(device, Arc::new(MockCerts))
            .host(custom_host)
            .port(proxy_port)
            .provider_url(provider_url)
            .build();

        let (tx, rx) = oneshot::channel();

        let proxy_handle = tokio::spawn(async move { proxy.run_server(tx).await });

        let updated_proxy = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout waiting for proxy to start")
            .expect("Failed to receive updated proxy");

        // Verify the proxy is running with custom settings
        let client = reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let response = client
            .get(format!(
                "https://{}:{}/api/version",
                custom_host, proxy_port
            ))
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(response.status(), 200);

        // Cleanup
        updated_proxy.stop(true).await;
        proxy_handle.abort();
        mock_llm_handle.stop(true).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
