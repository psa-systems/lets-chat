# List available recipes
default:
    @just --list

# Run all checks
check: check-server check-server-saas check-desktop check-clippy check-clippy-saas check-fmt

# Check server compilation (standalone)
check-server:
    ./dev/cargo check -p lets-chat-server

# Check server compilation (saas)
check-server-saas:
    ./dev/cargo check -p lets-chat-server --no-default-features --features saas

# Check desktop compilation
check-desktop:
    ./dev/cargo-desktop check -p lets-chat-desktop

# Run clippy lints (standalone server + desktop)
check-clippy:
    ./dev/cargo clippy -p lets-chat-server
    ./dev/cargo-desktop clippy -p lets-chat-desktop

# Run clippy lints (saas server)
check-clippy-saas:
    ./dev/cargo clippy -p lets-chat-server --no-default-features --features saas

# Check formatting
check-fmt:
    ./dev/cargo fmt --check

# Build Docker image for validation (standalone)
check-docker:
    docker buildx build --tag lets-chat:check -f ci-build/Dockerfile.web .

# Build Docker image for validation (saas)
check-docker-saas:
    docker buildx build --tag lets-chat-saas:check --build-arg BUILD_MODE=saas -f ci-build/Dockerfile.web .

# Build Tailwind CSS from source
build-css:
    cd server && ../dev/bun install --frozen-lockfile
    cd server && ../dev/bun run tailwindcss --input assets/tailwind.css --output assets/tailwind-built.css --minify

# Build release binary (standalone)
build: build-css
    ./dev/cargo build --release -p lets-chat-server

# Build release binary (saas)
build-saas: build-css
    ./dev/cargo build --release -p lets-chat-server --no-default-features --features saas

# Build Docker image (standalone)
build-docker: build-css
    docker buildx build --tag lets-chat:local -f ci-build/Dockerfile.web .

# Build Docker image (saas)
build-docker-saas: build-css
    docker buildx build --tag lets-chat-saas:local --build-arg BUILD_MODE=saas -f ci-build/Dockerfile.web .

# Start development server (web, standalone) via Docker with Traefik
dev-web:
    @echo "Web: https://{{ env('USER') }}-chat.a8n.run"
    BUILD_MODE=standalone docker compose -f compose.dev.yml up --build

# Start development server (web, saas) via Docker with Traefik
dev-web-saas:
    @echo "Web: https://{{ env('USER') }}-chat.a8n.run"
    BUILD_MODE=saas docker compose -f compose.dev.yml up --build

# Stop dev-web containers
dev-web-down:
    docker compose -f compose.dev.yml down

# Stop dev-web containers and remove volumes
dev-web-clean:
    docker compose -f compose.dev.yml down -v

# Start development server (web, standalone) locally on http://localhost:18080
dev-web-local: build-css
    HOST_PORT=18080 ./dev/server-up -p lets-chat-server

# Start development server (web, saas) locally on http://localhost:18080
dev-web-local-saas: build-css
    HOST_PORT=18080 ./dev/server-up -p lets-chat-server --no-default-features --features saas

# Stop the local dev server container
dev-web-local-down:
    ./dev/server-down

# Start development server (desktop)
dev-desktop:
    LETS_CHAT_SERVER_URL=http://localhost:18080 ./dev/cargo-desktop run -p lets-chat-desktop

# Run tests (server, standalone)
test:
    ./dev/cargo test -p lets-chat-server

# Run tests (server, saas)
test-saas:
    ./dev/cargo test -p lets-chat-server --no-default-features --features saas

# Verify the standalone server binary starts and serves the login page
verify: build-css
    #!/usr/bin/env nu
    let container = "lets-chat-rewrite-server"
    print "Building release binary..."
    ./dev/cargo build --release -p lets-chat-server
    print ""
    print "Starting server container on port 18080..."
    with-env { HOST_PORT: "18080" } { ./dev/server-up --release -p lets-chat-server }
    # Wait for the server to be listening (poll for up to 30 seconds).
    mut http_code = "000"
    mut body = ""
    for i in 0..30 {
        sleep 1sec
        let alive = (try { ^docker inspect --format '{{{{.State.Running}}' $container | str trim } catch { "false" })
        if $alive != "true" {
            continue
        }
        $http_code = (try { ^curl --silent --output /dev/null --write-out '%{http_code}' http://127.0.0.1:18080/login } catch { "000" })
        if $http_code == "200" {
            $body = (try { ^curl --silent http://127.0.0.1:18080/login } catch { "" })
            break
        }
    }
    if $http_code != "200" {
        print "FAIL: Server did not become healthy within 30 seconds"
        try { ^docker logs --tail 80 $container }
        ./dev/server-down | ignore
        exit 1
    }
    ./dev/server-down | ignore
    if $http_code == "200" and ($body | str contains '<form') {
        print $"PASS: Server responded with HTTP ($http_code) and HTML form body"
    } else {
        print $"FAIL: Server responded with HTTP ($http_code), body did not contain '<form'"
        exit 1
    }

# Format code
fmt:
    ./dev/cargo fmt --all
