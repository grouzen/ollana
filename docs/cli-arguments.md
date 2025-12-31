# CLI Arguments Documentation

## Multi-Provider Support

Ollana now supports multiple LLM providers including Ollama, vLLM, LM Studio, and llama.cpp server. This document describes the CLI arguments for configuring multi-provider support.

## Core Arguments

### `--allowed-providers <PROVIDERS>`

Comma-separated list of allowed provider types to discover and serve.

**Valid provider names:**
- `ollama` - Ollama server
- `vllm` - vLLM server
- `lm-studio` (or `lmstudio`) - LM Studio server
- `llama-server` (or `llamaserver`) - llama.cpp server

**Default:** All providers are allowed if not specified.

**Examples:**
```bash
# Allow only Ollama
ollana serve --allowed-providers ollama

# Allow Ollama and vLLM
ollana serve --allowed-providers ollama,vllm

# Allow all providers (explicitly)
ollana serve --allowed-providers ollama,vllm,lm-studio,llama-server
```

## Port Mapping Arguments

Each provider has a dedicated port mapping argument that controls how ports are configured in both server and client modes.

### Port Mapping Format

Port mappings support two formats:

1. **Two ports:** `<port1>:<port2>`
2. **Single port:** `<port>` (equivalent to `<port>:<port+1>`)
3. **Discovery mode:** `:<port2>` (client-only, discovers port1 from network)

### Port Mapping Semantics

The meaning of `port1` and `port2` differs between server and client modes:

#### Server Mode
- `port1`: The actual LLM server port (where the provider runs)
- `port2`: The Ollana server proxy port (where Ollana listens)
- **Binding:** Server proxy binds to `0.0.0.0:<port2>` (network-accessible)

#### Client Mode
- `port1`: The server proxy port to connect to (from discovery or explicit)
- `port2`: The client proxy HTTP server port (where local clients connect)
- **Binding:** Client proxy binds to `127.0.0.1:<port2>` (localhost-only)
- **Discovery:** If `port1` is omitted (`:port2` format), it's taken from discovery response

### Provider-Specific Arguments

#### `--ollama-ports <PORT_MAPPING>`

Configure Ollama port mapping.

**Default ports:**
- LLM server: `11434`
- Server proxy: `11435` (LLM port + 1)
- Client proxy: `11434` (matches LLM default for compatibility)

**Examples:**
```bash
# Server: Ollama on 11434, proxy on 11435
ollana serve --ollama-ports 11434

# Server: Ollama on 11434, proxy on 8888
ollana serve --ollama-ports 11434:8888

# Client: Discover server proxy port, expose on 11434
ollana serve --ollama-ports :11434

# Client: Connect to server proxy on 8888, expose on 11435
ollana serve --ollama-ports 8888:11435
```

#### `--vllm-ports <PORT_MAPPING>`

Configure vLLM port mapping.

**Default ports:**
- LLM server: `8000`
- Server proxy: `8001` (LLM port + 1)
- Client proxy: `8000` (matches LLM default)

**Examples:**
```bash
# Server: vLLM on 8000, proxy on 8001
ollana serve --vllm-ports 8000

# Server: vLLM on 9000, proxy on 9999
ollana serve --vllm-ports 9000:9999

# Client: Discover server proxy port, expose on 8002
ollana serve --vllm-ports :8002
```

#### `--lmstudio-ports <PORT_MAPPING>`

Configure LM Studio port mapping.

**Default ports:**
- LLM server: `1234`
- Server proxy: `1235` (LLM port + 1)
- Client proxy: `1234` (matches LLM default)

**Examples:**
```bash
# Server: LM Studio on 1234, proxy on 1235
ollana serve --lmstudio-ports 1234

# Client: Discover server proxy port, expose on 1234
ollana serve --lmstudio-ports :1234
```

#### `--llama-server-ports <PORT_MAPPING>`

Configure llama.cpp server port mapping.

**Default ports:**
- LLM server: `8080`
- Server proxy: `8081` (LLM port + 1)
- Client proxy: `8080` (matches LLM default)

**Examples:**
```bash
# Server: llama.cpp on 8080, proxy on 8081
ollana serve --llama-server-ports 8080

# Server: llama.cpp on 9090, proxy on 9091
ollana serve --llama-server-ports 9090:9091
```

## Complete Examples

### Server Mode Examples

```bash
# Minimal server - auto-detect Ollama on default port
ollana serve --force-server-mode

# Server with multiple providers, default ports
ollana serve \
  --allowed-providers ollama,vllm \
  --force-server-mode

# Server with custom port mappings
ollana serve \
  --allowed-providers ollama,vllm \
  --ollama-ports 11434:8888 \
  --vllm-ports 8000:8001 \
  --force-server-mode

# Server with single custom Ollama port
ollana serve \
  --allowed-providers ollama \
  --ollama-ports 11434 \
  --force-server-mode
```

### Client Mode Examples

```bash
# Client allowing all providers (default ports)
ollana serve --allowed-providers ollama,vllm,lm-studio,llama-server

# Client with custom Ollama client proxy port
ollana serve \
  --allowed-providers ollama \
  --ollama-ports :11435

# Client with explicit server proxy connection and custom client port
ollana serve \
  --allowed-providers ollama,vllm \
  --ollama-ports 8888:11434 \
  --vllm-ports 8001:8002
```

### Mixed Examples

```bash
# Development setup - run both client and server
# Terminal 1 (server):
ollana serve \
  --allowed-providers ollama \
  --ollama-ports 11434:8888 \
  --force-server-mode

# Terminal 2 (client):
ollana serve \
  --allowed-providers ollama \
  --ollama-ports 8888:11434
```

## Port Collision Handling

If a port is already in use, Ollana will fail to bind and exit with an error. There is **no automatic port reassignment**.

**Resolution:**
1. Identify the conflicting process: `lsof -i :<port>` or `netstat -tulpn | grep <port>`
2. Either stop the conflicting process or use different ports via explicit mappings

**Example:**
```bash
# Port 11434 is busy
ollana serve --ollama-ports 11434:8888
# Error: Address already in use (os error 98)

# Solution: Use different LLM port or client proxy port
ollana serve --ollama-ports :11435  # Client mode, different client proxy port
```

## Best Practices

1. **Server mode:** Always use `--force-server-mode` to prevent auto-switching to client mode
2. **Client mode:** Use discovery mode (`:port`) when possible to automatically find server proxies
3. **Development:** Use explicit port mappings for predictable configurations
4. **Production:** Document your port configuration in `config.toml` (CLI overrides config file)
5. **Firewall:** Remember that server proxies bind to `0.0.0.0` (all interfaces) while client proxies bind to `127.0.0.1` (localhost only)

## Default Port Reference

| Provider | LLM Port | Default Server Proxy | Default Client Proxy |
|----------|----------|---------------------|---------------------|
| Ollama | 11434 | 11435 | 11434 |
| vLLM | 8000 | 8001 | 8000 |
| LM Studio | 1234 | 1235 | 1234 |
| llama.cpp | 8080 | 8081 | 8080 |

## Configuration File Support

All CLI arguments can also be specified in `config.toml`. CLI arguments take precedence over config file values.

Example `config.toml`:
```toml
[serve]
allowed_providers = ["ollama", "vllm"]
ollama_ports = "11434:8888"
vllm_ports = "8000:8001"
```

Then run with config:
```bash
# Uses config.toml defaults
ollana serve --force-server-mode

# Override specific values
ollana serve --force-server-mode --ollama-ports 11434:9999
```
