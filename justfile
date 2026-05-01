# List available recipes
default:
    @just --list

# Run all checks
check: check-server check-desktop check-clippy check-fmt

# Check server compilation
check-server:
    ./dev/cargo check -p lets-chat-server

# Check desktop compilation
check-desktop:
    ./dev/cargo-desktop check -p lets-chat-desktop

# Run clippy lints (server in the slim image, desktop in the GTK-equipped image)
check-clippy:
    ./dev/cargo clippy -p lets-chat-server
    ./dev/cargo-desktop clippy -p lets-chat-desktop

# Check formatting
check-fmt:
    ./dev/cargo fmt --check

# Build Docker image for validation
check-docker:
    docker buildx build --tag lets-chat:check -f ci-build/Dockerfile.web .

# Build Tailwind CSS from source
build-css:
    cd server && ../dev/bun install --frozen-lockfile
    cd server && ../dev/bun run tailwindcss --input assets/tailwind.css --output assets/tailwind-built.css --minify

# Build release binary
build: build-css
    ./dev/cargo build --release -p lets-chat-server

# Build Docker image
build-docker: build-css
    docker buildx build --tag lets-chat:local -f ci-build/Dockerfile.web .

# Start development server (web) via Docker with Traefik
dev-web:
    @echo "Web: https://{{ env('USER') }}-chat.a8n.run"
    docker compose -f compose.dev.yml up --build

# Stop dev-web containers
dev-web-down:
    docker compose -f compose.dev.yml down

# Stop dev-web containers and remove volumes
dev-web-clean:
    docker compose -f compose.dev.yml down -v

# Start development server (web) locally on http://localhost:18080
dev-web-local: build-css
    HOST_PORT=18080 ./dev/server-up -p lets-chat-server

# Stop the local dev server container
dev-web-local-down:
    ./dev/server-down

# Start development server (desktop)
dev-desktop:
    LETS_CHAT_SERVER_URL=http://localhost:18080 ./dev/cargo-desktop run -p lets-chat-desktop

# Run tests (server only; the desktop crate has no tests and needs the GTK image)
test:
    ./dev/cargo test -p lets-chat-server

# Verify the server binary starts and serves the login page
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
