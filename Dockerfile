FROM rust:1.93-slim-trixie

RUN apt-get update && apt-get install --yes --no-install-recommends \
    curl pkg-config libssl-dev unzip \
    && rm -rf /var/lib/apt/lists/*

# Install Bun for Tailwind CSS
RUN curl --fail --location --silent --show-error https://bun.sh/install | bash
ENV PATH="/root/.bun/bin:${PATH}"

RUN curl --location --silent --show-error https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-unknown-linux-gnu.tgz \
    | tar --extract --gzip --directory /usr/local/cargo/bin
RUN cargo binstall dioxus-cli@0.7.5 --no-confirm

RUN rustup target add wasm32-unknown-unknown

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build && rm -rf src

# Install npm deps for Tailwind CSS
COPY package.json tailwind.config.js ./
RUN bun install

EXPOSE 8080

# Build CSS at startup (assets/ is volume-mounted, so must build after mount)
CMD ["sh", "-c", "bun run tailwindcss --input assets/tailwind.css --output assets/tailwind-built.css --minify && exec dx serve --platform web --addr 0.0.0.0 --port 8080"]
