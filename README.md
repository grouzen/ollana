# Ollana (Ollama Over LAN)

**Note: it is still in the early stage of development.**

Auto-discover your Ollama server on your local network with hassle-free ease.

Use your home or office Ollama server from any device on the same network without changing settings in your client applications or setting up a reverse proxy.

## Usage

### Serve

Run a proxy.
It automatically detects the mode (client or server) to run in by checking whether an Ollama server is running on your machine.

```shell
ollana serve
```

It also support an old-style SysV daemon mode to run in a background:
```shell
ollana serve -d
```

## Architecture

![Architecture Overview](docs/architecture-overview.png)

See also [architecture-overview.md](docs/architecture-overview.md) for more details.


## Contributing

Auto-reloading development server (see: https://actix.rs/docs/autoreload)

```shell
watchexec -e rs -r cargo run
```

### Debugging

You can debug the application by setting the `RUST_LOG` environment variable to the desired level of verbosity. For example, to enable debug level:
```shell
RUST_LOG=debug ollana serve
```
