use std::net::SocketAddr;

use url::Url;

use crate::constants;

#[async_trait::async_trait]
pub trait Provider {
    async fn health_check(&self) -> anyhow::Result<bool>;

    fn get_port(&self) -> u16;
}

/// Configuration trait for LLM server providers
pub trait ProviderConfig {
    const DEFAULT_ADDRESS: &'static str;
    const DEFAULT_PORT: u16;

    fn health_check_path() -> &'static str;
}

/// Generic LLM server implementation
pub struct LLMServer<C: ProviderConfig> {
    client: reqwest::Client,
    url: Url,
    port: u16,
    _phantom: std::marker::PhantomData<C>,
}

impl<C: ProviderConfig> Default for LLMServer<C> {
    fn default() -> Self {
        let url = format!("http://{}:{}", C::DEFAULT_ADDRESS, C::DEFAULT_PORT);
        let url = Url::parse(&url).unwrap();
        let client = reqwest::Client::default();

        Self {
            client,
            url,
            port: C::DEFAULT_PORT,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<C: ProviderConfig> LLMServer<C> {
    pub fn new(socket_addr: SocketAddr, secure: bool) -> anyhow::Result<Self> {
        let url_schema = if secure { "https" } else { "http" };
        let url = format!("{}://{}", url_schema, socket_addr);
        let url = Url::parse(&url)?;
        let client = reqwest::ClientBuilder::new()
            .use_rustls_tls()
            .danger_accept_invalid_certs(true)
            .build()?;

        Ok(LLMServer {
            client,
            url,
            port: socket_addr.port(),
            _phantom: std::marker::PhantomData,
        })
    }
}

#[async_trait::async_trait]
impl<C: ProviderConfig + Sync> Provider for LLMServer<C> {
    async fn health_check(&self) -> anyhow::Result<bool> {
        let mut uri = self.url.clone();
        uri.set_path(C::health_check_path());

        self.client
            .get(uri)
            .send()
            .await
            .map(|response| response.status().is_success())
            .map_err(anyhow::Error::new)
    }

    fn get_port(&self) -> u16 {
        self.port
    }
}

/// Configuration for llama.cpp server
pub struct LlamaServerConfig;

impl ProviderConfig for LlamaServerConfig {
    const DEFAULT_ADDRESS: &'static str = constants::LLAMA_SERVER_DEFAULT_ADDRESS;
    const DEFAULT_PORT: u16 = constants::LLAMA_SERVER_DEFAULT_PORT;

    fn health_check_path() -> &'static str {
        "/health"
    }
}

/// Configuration for Ollama server
pub struct OllamaConfig;

impl ProviderConfig for OllamaConfig {
    const DEFAULT_ADDRESS: &'static str = constants::OLLAMA_DEFAULT_ADDRESS;
    const DEFAULT_PORT: u16 = constants::OLLAMA_DEFAULT_PORT;

    fn health_check_path() -> &'static str {
        "/"
    }
}

/// Configuration for LM Studio server
pub struct LMStudioConfig;

impl ProviderConfig for LMStudioConfig {
    const DEFAULT_ADDRESS: &'static str = constants::LMSTUDIO_DEFAULT_ADDRESS;
    const DEFAULT_PORT: u16 = constants::LMSTUDIO_DEFAULT_PORT;

    fn health_check_path() -> &'static str {
        "/v1/models"
    }
}

/// Configuration for vLLM server
pub struct VLLMConfig;

impl ProviderConfig for VLLMConfig {
    const DEFAULT_ADDRESS: &'static str = constants::VLLM_DEFAULT_ADDRESS;
    const DEFAULT_PORT: u16 = constants::VLLM_DEFAULT_PORT;

    fn health_check_path() -> &'static str {
        "/health"
    }
}

/// Type alias for llama.cpp server
pub type LlamaServer = LLMServer<LlamaServerConfig>;

/// Type alias for Ollama server
pub type Ollama = LLMServer<OllamaConfig>;

/// Type alias for LM Studio server
pub type LMStudio = LLMServer<LMStudioConfig>;

/// Type alias for vLLM server
pub type VLLM = LLMServer<VLLMConfig>;
