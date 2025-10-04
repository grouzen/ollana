use actix_cors::Cors;
use actix_web::{dev::ServerHandle, error, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use futures_util::StreamExt as _;
use log::error;
use std::{fs::File, io::BufReader, net::SocketAddr};
use tokio::sync::{mpsc, oneshot::Sender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;

use crate::{certs::Certs, constants};

pub const PROXY_DEFAULT_WORKERS_NUMBER: usize = 2;

#[derive(Clone)]
pub struct ClientProxy {
    client: reqwest::Client,
    host: String,
    port: u16,
    server_url: Url,
    handle: Option<ServerHandle>,
}

pub struct ServerProxy {
    client: reqwest::Client,
    host: String,
    port: u16,
    ollama_url: Url,
}

impl Default for ClientProxy {
    fn default() -> Self {
        let server_url = format!(
            "https://{}:{}",
            constants::OLLANA_SERVER_PROXY_DEFAULT_ADDRESS,
            constants::OLLANA_SERVER_PROXY_DEFAULT_PORT
        );
        let server_url = Url::parse(&server_url).unwrap();
        let client = reqwest::Client::default();

        Self {
            client,
            host: constants::OLLANA_CLIENT_PROXY_DEFAULT_ADDRESS.to_string(),
            port: constants::OLLANA_CLIENT_PROXY_DEFAULT_PORT,
            server_url,
            handle: None,
        }
    }
}

impl Default for ServerProxy {
    fn default() -> Self {
        let ollama_url = format!(
            "http://{}:{}",
            constants::OLLAMA_DEFAULT_ADDRESS,
            constants::OLLAMA_DEFAULT_PORT
        );
        let ollama_url = Url::parse(&ollama_url).unwrap();

        Self {
            client: reqwest::Client::default(),
            host: constants::OLLANA_SERVER_PROXY_DEFAULT_ADDRESS.to_string(),
            port: constants::OLLANA_SERVER_PROXY_DEFAULT_PORT,
            ollama_url,
        }
    }
}

impl ClientProxy {
    pub fn from_server_socket_addr(server_socket_addr: SocketAddr) -> anyhow::Result<Self> {
        let server_url = format!("https://{server_socket_addr}");
        let server_url = Url::parse(&server_url)?;
        let client = reqwest::ClientBuilder::new()
            .use_rustls_tls()
            .danger_accept_invalid_certs(true)
            .build()?;

        Ok(ClientProxy {
            client,
            server_url,
            ..Default::default()
        })
    }

    pub async fn run_server(&mut self, tx: Sender<Self>) -> anyhow::Result<()> {
        let client = self.client.clone();
        let server_url = self.server_url.clone();

        let server = HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(client.clone()))
                .app_data(web::Data::new(server_url.clone()))
                .wrap(Cors::permissive())
                .default_service(web::to(Self::forward))
        })
        .bind((self.host.clone(), self.port))?
        .workers(PROXY_DEFAULT_WORKERS_NUMBER)
        .run();

        let handle = server.handle();
        self.handle = Some(handle);

        if tx.send(self.clone()).is_err() {
            error!("Couldn't send an updated client proxy");
        }

        server.await.map_err(anyhow::Error::new)
    }

    async fn forward(
        req: HttpRequest,
        client: web::Data<reqwest::Client>,
        server_url: web::Data<Url>,
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

    pub async fn stop(&self, graceful: bool) {
        if let Some(handle) = &self.handle {
            handle.stop(graceful).await
        }
    }
}

impl ServerProxy {
    pub async fn run_server(&self, certs: &Certs) -> anyhow::Result<()> {
        let client = self.client.clone();
        let ollama_url = self.ollama_url.clone();

        let (cert_file, key_file) = certs.get_server_files()?;
        let rustls_config = Self::rustls_config(cert_file, key_file)?;

        HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(client.clone()))
                .app_data(web::Data::new(ollama_url.clone()))
                .default_service(web::to(Self::forward))
        })
        .bind_rustls_0_23((self.host.clone(), self.port), rustls_config)?
        .workers(PROXY_DEFAULT_WORKERS_NUMBER)
        .run()
        .await
        .map_err(anyhow::Error::new)
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

    async fn forward(
        req: HttpRequest,
        client: web::Data<reqwest::Client>,
        ollama_url: web::Data<Url>,
        mut payload: web::Payload,
        method: actix_web::http::Method,
    ) -> Result<HttpResponse, Error> {
        let (tx, rx) = mpsc::unbounded_channel();

        actix_web::rt::spawn(async move {
            while let Some(chunk) = payload.next().await {
                tx.send(chunk).unwrap();
            }
        });

        let mut ollama_uri = (**ollama_url).clone();
        ollama_uri.set_path(req.uri().path());
        ollama_uri.set_query(req.uri().query());

        let ollama_request = client
            .request(
                reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
                ollama_uri,
            )
            .body(reqwest::Body::wrap_stream(UnboundedReceiverStream::new(rx)));

        let ollama_response = ollama_request
            .send()
            .await
            .map_err(error::ErrorInternalServerError)?;

        let mut response = HttpResponse::build(
            actix_web::http::StatusCode::from_u16(ollama_response.status().as_u16()).unwrap(),
        );

        Ok(response.streaming(ollama_response.bytes_stream()))
    }
}
