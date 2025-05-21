use std::io;

use ollana::client_proxy::ClientProxy;

#[actix_web::main]
async fn main() -> io::Result<()> {
    // To run the proxy we need:
    // 1. check if there is no ollama running on localhost:port where port is a default or configured ollama port
    //    a. in such case, start ServerProxy
    //    b. otherwise, start ClientProxy
    let client_proxy = ClientProxy::default();

    client_proxy.run_server().await
}
