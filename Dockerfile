FROM rust:1.93-slim-trixie

RUN apt-get update && apt-get install --yes --no-install-recommends \
    curl pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl --location --silent --show-error https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-unknown-linux-gnu.tgz \
    | tar --extract --gzip --directory /usr/local/cargo/bin
RUN cargo binstall dioxus-cli@0.7.3 --no-confirm

RUN rustup target add wasm32-unknown-unknown

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build && rm -rf src

EXPOSE 8080

CMD ["dx", "serve"]
