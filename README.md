# Let's Chat

A fullstack chat application built with Dioxus (Rust + WASM) and Axum.

## Quick Start

### Prerequisites

- Rust toolchain with `wasm32-unknown-unknown` target
- [Dioxus CLI](https://dioxuslabs.com/) v0.7.3+
- [Bun](https://bun.sh/) (for Tailwind CSS)

### Development

```nu
# Install dependencies and build Tailwind CSS
bun install
bun run tailwindcss --input assets/tailwind.css --output assets/tailwind-built.css

# Run the web app in dev mode
dx serve --platform web
```

### Docker

```nu
docker build --tag lets-chat --file ci-build/Dockerfile.web .
docker run --publish 8080:8080 --volume lets-chat-data:/data lets-chat
```

## Environment Variables

| Variable | Required | Description |
|---|---|---|
| `LETS_CHAT_DATA_DIR` | No | Data directory (default: `/data`) |
| `RUST_LOG` | No | Log level filter (default: `lets_chat=info`) |
| `BIND_ADDR` | No | Server listen address (default: `0.0.0.0:8080`) |
