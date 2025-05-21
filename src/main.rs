use std::io;

use ollana::{client_proxy::ClientProxy, server_proxy::ServerProxy};

#[actix_web::main]
async fn main() -> io::Result<()> {
    // To run the proxy we need:
    // 1. check if there is no ollama running on localhost:port where port is a default or configured ollama port
    //    a. in such case, start ServerProxy
    //    b. otherwise, start ClientProxy
    // let client_proxy = ClientProxy::default();
    let server_proxy = ServerProxy::default();

    // client_proxy.run_server().await
    server_proxy.run_server().await
}
