use actix_web::{error, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use futures_util::StreamExt as _;
use std::{io, net::ToSocketAddrs};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;

use crate::constants;

pub struct ServerProxy {
    pub host: String,
    pub port: u16,
    pub ollama_host: String,
    pub ollama_port: u16,
}

impl Default for ServerProxy {
    fn default() -> Self {
        Self {
            host: constants::OLLANA_SERVER_PROXY_DEFAULT_ADDRESS.to_string(),
            port: constants::OLLANA_SERVER_PROXY_DEFAULT_PORT,
            ollama_host: constants::OLLAMA_DEFAULT_ADDRESS.to_string(),
            ollama_port: constants::OLLAMA_DEFAULT_PORT,
        }
    }
}

impl ServerProxy {
    pub async fn run_server(&self) -> io::Result<()> {
        let client = reqwest::Client::default();
        let ollama_socket_addr = (self.ollama_host.clone(), self.ollama_port)
            .to_socket_addrs()?
            .next()
            .expect("server proxy address is invalid");
        let ollama_url = format!("http://{ollama_socket_addr}");
        let ollama_url = Url::parse(&ollama_url).unwrap();

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

        let server_req = client
            .request(
                reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
                ollama_uri,
            )
            .body(reqwest::Body::wrap_stream(UnboundedReceiverStream::new(rx)));

        let ollama_response = server_req
            .send()
            .await
            .map_err(error::ErrorInternalServerError)?;

        let mut response = HttpResponse::build(
            actix_web::http::StatusCode::from_u16(ollama_response.status().as_u16()).unwrap(),
        );

        Ok(response.streaming(ollama_response.bytes_stream()))
    }
}
