use actix_web::{error, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use futures_util::StreamExt as _;
use std::{io, net::ToSocketAddrs};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;

pub struct ClientProxy {
    pub client_host: String,
    pub client_port: u16,
    // TODO: determine via UDP broadcasting discovery
    pub server_host: String,
    pub server_port: u16,
}

const OLLAMA_DEFAULT_PORT: u16 = 11434;
const OLLANA_CLIENT_PROXY_DEFAULT_ADDRESS: &str = "127.0.0.1";
const OLLANA_SERVER_PROXY_DEFAULT_ADDRESS: &str = "127.0.0.1";
const OLLANA_CLIENT_PROXY_DEFAULT_PORT: u16 = OLLAMA_DEFAULT_PORT;
const OLLANA_SERVER_PROXY_DEFAULT_PORT: u16 = 11435;

impl Default for ClientProxy {
    fn default() -> Self {
        Self {
            client_host: OLLANA_CLIENT_PROXY_DEFAULT_ADDRESS.to_string(),
            client_port: OLLANA_CLIENT_PROXY_DEFAULT_PORT,
            server_host: OLLANA_SERVER_PROXY_DEFAULT_ADDRESS.to_string(),
            server_port: OLLANA_SERVER_PROXY_DEFAULT_PORT,
        }
    }
}

// http proxy that listens on the default ollama port and passes
// the requests to a remote ollana server port
impl ClientProxy {
    pub async fn run_server(&self) -> io::Result<()> {
        let client = reqwest::Client::default();
        let server_socket_addr = (self.server_host.clone(), self.server_port)
            .to_socket_addrs()?
            .next()
            .expect("server proxy address is invalid");
        let server_url = format!("http://{server_socket_addr}");
        let server_url = Url::parse(&server_url).unwrap();

        HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(client.clone()))
                .app_data(web::Data::new(server_url.clone()))
                .default_service(web::to(Self::forward))
        })
        .bind((self.client_host.clone(), self.client_port))?
        .run()
        .await
    }

    async fn forward(
        req: HttpRequest,
        client: web::Data<reqwest::Client>,
        server_url: web::Data<Url>,
        mut payload: web::Payload,
        method: actix_web::http::Method,
    ) -> Result<HttpResponse, Error> {
        let (tx, rx) = mpsc::unbounded_channel();

        actix_web::rt::spawn(async move {
            while let Some(chunk) = payload.next().await {
                tx.send(chunk).unwrap();
            }
        });

        let mut server_uri = (**server_url).clone();
        server_uri.set_path(req.uri().path());
        server_uri.set_query(req.uri().query());

        let server_req = client
            .request(
                reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
                server_uri,
            )
            .body(reqwest::Body::wrap_stream(UnboundedReceiverStream::new(rx)));

        let server_response = server_req
            .send()
            .await
            .map_err(error::ErrorInternalServerError)?;

        let mut response = HttpResponse::build(
            actix_web::http::StatusCode::from_u16(server_response.status().as_u16()).unwrap(),
        );

        Ok(response.streaming(server_response.bytes_stream()))
    }
}
