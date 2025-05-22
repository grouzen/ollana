use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use futures_util::StreamExt as _;
use std::{io, net::ToSocketAddrs};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;

use crate::{constants, error::OllanaError};

pub struct ClientProxy {
    client: reqwest::Client,
    host: String,
    port: u16,
    // TODO: determine via UDP broadcasting discovery
    server_url: Url,
}

impl Default for ClientProxy {
    fn default() -> Self {
        let server_url = format!(
            "http://{}:{}",
            constants::OLLANA_SERVER_PROXY_DEFAULT_ADDRESS,
            constants::OLLANA_SERVER_PROXY_DEFAULT_PORT
        );
        let server_url = Url::parse(&server_url).unwrap();

        Self {
            client: reqwest::Client::default(),
            host: constants::OLLANA_CLIENT_PROXY_DEFAULT_ADDRESS.to_string(),
            port: constants::OLLANA_CLIENT_PROXY_DEFAULT_PORT,
            server_url: server_url,
        }
    }
}

// http proxy that listens on the default ollama port and passes
// the requests to a remote ollana server port
impl ClientProxy {
    pub fn try_new(server_host: String, server_port: u16) -> crate::error::Result<ClientProxy> {
        let server_socket_addr = (server_host, server_port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| OllanaError::Other("Server proxy address is invalid".to_string()))?;
        let server_url = format!("http://{server_socket_addr}");
        let server_url = Url::parse(&server_url).map_err(OllanaError::from)?;

        Ok(ClientProxy {
            server_url: server_url,
            ..Default::default()
        })
    }

    pub async fn run_server(&self) -> io::Result<()> {
        let client = self.client.clone();
        let server_url = self.server_url.clone();

        HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(client.clone()))
                .app_data(web::Data::new(server_url.clone()))
                .default_service(web::to(Self::forward))
        })
        .bind((self.host.clone(), self.port))?
        .run()
        .await
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

        let ollana_server_req = client
            .request(
                reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
                server_uri,
            )
            .body(reqwest::Body::wrap_stream(UnboundedReceiverStream::new(rx)));

        let ollana_server_response = ollana_server_req
            .send()
            .await
            .map_err(actix_web::error::ErrorInternalServerError)?;

        let mut response = HttpResponse::build(
            actix_web::http::StatusCode::from_u16(ollana_server_response.status().as_u16())
                .unwrap(),
        );

        Ok(response.streaming(ollana_server_response.bytes_stream()))
    }
}
