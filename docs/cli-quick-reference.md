# CLI Arguments Quick Reference

## Flag Syntax

```bash
ollana serve [FLAGS]

FLAGS:
  --allowed-providers <PROVIDERS>        # ollama,vllm,lm-studio,llama-server
  --ollama-ports <MAPPING>              # <port> or <port1>:<port2> or :<port2>
  --vllm-ports <MAPPING>                # <port> or <port1>:<port2> or :<port2>
  --lmstudio-ports <MAPPING>            # <port> or <port1>:<port2> or :<port2>
  --llama-server-ports <MAPPING>        # <port> or <port1>:<port2> or :<port2>
```

## Port Mapping Cheat Sheet

| Format | Server Mode | Client Mode |
|--------|-------------|-------------|
| `11434` | LLM=11434, Proxy=11435 | ServerProxy=11434, ClientProxy=11435 |
| `11434:8888` | LLM=11434, Proxy=8888 | ServerProxy=11434, ClientProxy=8888 |
| `:8888` | ❌ Invalid | Discover ServerProxy, ClientProxy=8888 |

## Common Commands

```bash
# Server: Single provider, default ports
ollana serve --allowed-providers ollama --force-server-mode

# Server: Custom Ollama proxy port
ollana serve --ollama-ports 11434:8888 --force-server-mode

# Client: Discover all providers
ollana serve --allowed-providers ollama,vllm,lm-studio,llama-server

# Client: Custom client proxy port
ollana serve --ollama-ports :11435

# Both: Multiple providers with custom ports
ollana serve \
  --allowed-providers ollama,vllm \
  --ollama-ports 11434:8888 \
  --vllm-ports 8000:9000
```

## Default Ports

| Provider | LLM | Server Proxy | Client Proxy |
|----------|-----|--------------|--------------|
| Ollama | 11434 | 11435 | 11434 |
| vLLM | 8000 | 8001 | 8000 |
| LM Studio | 1234 | 1235 | 1234 |
| llama.cpp | 8080 | 8081 | 8080 |

## Error Messages

```bash
# Invalid provider
--allowed-providers invalid
# Error: Invalid provider type: invalid. Valid values: ollama, vllm, lm-studio, llama-server

# Invalid port
--ollama-ports 70000
# Error: Invalid port number: 70000. Must be between 0 and 65535

# Invalid format
--ollama-ports 11434:8888:9999
# Error: Invalid port mapping format: 11434:8888:9999. Expected <port1>:<port2> or <port>
```
