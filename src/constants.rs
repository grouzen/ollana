pub const OLLAMA_DEFAULT_ADDRESS: &str = "127.0.0.1";
pub const OLLAMA_DEFAULT_PORT: u16 = 11434;

pub const LLAMA_SERVER_DEFAULT_ADDRESS: &str = "127.0.0.1";
pub const LLAMA_SERVER_DEFAULT_PORT: u16 = 8080;

pub const LMSTUDIO_DEFAULT_ADDRESS: &str = "127.0.0.1";
pub const LMSTUDIO_DEFAULT_PORT: u16 = 1234;

pub const VLLM_DEFAULT_ADDRESS: &str = "127.0.0.1";
pub const VLLM_DEFAULT_PORT: u16 = 8000;

pub const OLLANA_CLIENT_PROXY_DEFAULT_ADDRESS: &str = "127.0.0.1";
/// Default port for client proxy when provider type is not specified
pub const OLLANA_CLIENT_PROXY_DEFAULT_PORT: u16 = OLLAMA_DEFAULT_PORT;

/// Get the default client proxy port for a given provider type
pub fn get_default_client_proxy_port(provider_type: crate::proto::ProviderType) -> u16 {
    use crate::proto::ProviderType;
    match provider_type {
        ProviderType::Ollama => OLLAMA_DEFAULT_PORT,
        ProviderType::Vllm => VLLM_DEFAULT_PORT,
        ProviderType::LmStudio => LMSTUDIO_DEFAULT_PORT,
        ProviderType::LlamaServer => LLAMA_SERVER_DEFAULT_PORT,
        _ => OLLAMA_DEFAULT_PORT, // fallback for unspecified
    }
}

pub const OLLANA_SERVER_PROXY_DEFAULT_ADDRESS: &str = "0.0.0.0";
pub const OLLANA_SERVER_PROXY_DEFAULT_PORT: u16 = 20110;

pub const OLLANA_SERVER_DEFAULT_DISCOVERY_PORT: u16 = 20111;
