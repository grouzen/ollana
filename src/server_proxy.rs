use actix_web::{error, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use futures_util::StreamExt as _;
use std::{io, net::ToSocketAddrs};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;

use crate::{constants, error::OllanaError};

pub struct ServerProxy {
    client: reqwest::Client,
    host: String,
    port: u16,
    ollama_url: Url,
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
            ollama_url: ollama_url,
        }
    }
}

impl ServerProxy {
    pub fn try_new(ollama_host: String, ollama_port: u16) -> crate::error::Result<Self> {
        let server_socket_addr = (ollama_host, ollama_port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| OllanaError::Other("Ollama address is invalid".to_string()))?;
        let ollama_url = format!("http://{server_socket_addr}");
        let ollama_url = Url::parse(&ollama_url).map_err(OllanaError::from)?;

        Ok(ServerProxy {
            ollama_url: ollama_url,
            ..Default::default()
        })
    }

    pub async fn run_server(&self) -> io::Result<()> {
        let client = self.client.clone();
        let ollama_url = self.ollama_url.clone();

        HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(client.clone()))
                .app_data(web::Data::new(ollama_url.clone()))
                .default_service(web::to(Self::forward))
        })
        .bind((self.host.clone(), self.port))?
        .run()
        .await
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
