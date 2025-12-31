# Configuration File Support

## Overview

Ollana supports configuration through both CLI arguments and a `config.toml` configuration file. This provides flexibility for users who prefer declarative configuration over command-line flags.

## Configuration File Location

The configuration file should be named `config.toml` and placed in the Ollana data directory:

- **Linux**: `~/.local/share/ollana/config.toml`
- **macOS**: `~/Library/Application Support/ollana/config.toml`

## Precedence Rules

Configuration values are resolved using the following precedence (highest to lowest):

1. **CLI Arguments** - Command-line flags always take precedence
2. **Configuration File** - Values from `config.toml`
3. **Defaults** - Built-in default values

This means you can set defaults in `config.toml` and override them on a per-invocation basis using CLI flags.

## Configuration Schema

The configuration file supports the following options:

### allowed_providers

List of provider types to allow for discovery and connection.

**Type**: Array of strings  
**Valid values**: `"ollama"`, `"vllm"`, `"lm-studio"`, `"llama-server"`  
**Default**: All providers allowed  
**CLI equivalent**: `--allowed-providers`

**Example:**
```toml
allowed_providers = ["ollama", "vllm"]
```

### Port Mappings

Port mappings define how Ollana maps between LLM server ports and proxy ports.

**Format**: `"<port1>:<port2>"`
- Server mode: `port1` = LLM server port, `port2` = Ollana server proxy port
- Client mode: `port1` = server proxy port to connect to, `port2` = client proxy port

**Partial formats supported:**
- `"<port1>:"` - Specify only port1, port2 will be auto-assigned or use default
- `":<port2>"` - Specify only port2, port1 will be taken from discovery or use default

#### ollama_ports

**Type**: String  
**Default**: `"11434:11435"` (server) / `"<discovered>:11434"` (client)  
**CLI equivalent**: `--ollama-ports`

**Example:**
```toml
ollama_ports = "11434:8888"
```

#### vllm_ports

**Type**: String  
**Default**: `"8000:8001"` (server) / `"<discovered>:8000"` (client)  
**CLI equivalent**: `--vllm-ports`

**Example:**
```toml
vllm_ports = "8000:8001"
```

#### lmstudio_ports

**Type**: String  
**Default**: `"1234:1235"` (server) / `"<discovered>:1234"` (client)  
**CLI equivalent**: `--lmstudio-ports`

**Example:**
```toml
lmstudio_ports = "1234:1235"
```

#### llama_server_ports

**Type**: String  
**Default**: `"8080:8081"` (server) / `"<discovered>:8080"` (client)  
**CLI equivalent**: `--llama-server-ports`

**Example:**
```toml
llama_server_ports = "8080:8081"
```

## Validation

The configuration system performs validation on both the config file and the merged configuration:

1. **Provider Type Validation**: Ensures all provider names are valid
2. **Port Mapping Validation**: Verifies port mapping format and port number ranges (0-65535)
3. **Port Conflict Detection**: Checks for duplicate port usage across providers

Validation errors will be reported at startup with clear error messages.

## Complete Examples

### Example 1: Simple Ollama-only Setup

**config.toml:**
```toml
allowed_providers = ["ollama"]
ollama_ports = "11434:8888"
```

This configuration:
- Only allows Ollama provider
- Server proxy runs on port 8888 when Ollama is on port 11434

### Example 2: Multi-Provider Setup

**config.toml:**
```toml
allowed_providers = ["ollama", "vllm", "lm-studio"]
ollama_ports = "11434:8888"
vllm_ports = "8000:8001"
lmstudio_ports = "1234:1235"
```

This configuration:
- Allows Ollama, vLLM, and LM Studio providers
- Each provider has custom server proxy ports

### Example 3: Client with Custom Local Ports

**config.toml:**
```toml
allowed_providers = ["ollama"]
ollama_ports = ":11435"
```

This configuration:
- Client will expose Ollama on localhost port 11435 instead of default 11434
- Server proxy port will be discovered via UDP broadcast

### Example 4: CLI Override

**config.toml:**
```toml
allowed_providers = ["ollama"]
ollama_ports = "11434:8888"
```

**Command:**
```bash
ollana serve --ollama-ports 11434:9999
```

**Result:**
- Ollama server proxy will use port 9999 (CLI overrides config file)
- Only Ollama provider allowed (from config file)

## Usage in Code

The configuration system is integrated into `ServeArgs`:

```rust
use ollana::args::ServeArgs;
use ollana::config::MergedConfig;

// Create merged config from CLI args and config file
let merged_config = serve_args.merged_config()?;

// Use the merged config
let allowed_providers = merged_config.get_allowed_providers();
let ollama_mapping = merged_config.get_port_mapping(ProviderType::Ollama);
```

## Error Handling

The configuration system provides clear error messages for common issues:

- **Missing config file**: Silently uses defaults (this is expected)
- **Invalid TOML syntax**: Reports parsing error with line/column information
- **Invalid provider name**: Lists valid provider names
- **Invalid port mapping**: Shows expected format
- **Port out of range**: Indicates valid port range (0-65535)
- **Port conflict**: Lists all providers using the conflicting port

## Migration from CLI-Only

If you're currently using CLI arguments, migration is straightforward:

1. Create `config.toml` in the data directory
2. Copy your CLI arguments to the config file using the appropriate format
3. Test that the configuration works: `ollana serve`
4. Remove CLI arguments from your scripts (optional - they still work)

The configuration file is **completely optional** - all existing CLI-based workflows continue to work without any changes.
