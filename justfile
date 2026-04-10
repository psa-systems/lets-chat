# List available recipes
default:
    @just --list

# Run all checks (server, web, clippy, fmt)
check: check-server check-web check-clippy check-fmt

# Check server compilation
check-server:
    cargo check

# Check web/WASM compilation
check-web:
    cargo check --target wasm32-unknown-unknown

# Run clippy lints
check-clippy:
    cargo clippy

# Check formatting
check-fmt:
    cargo fmt --check

# Build Docker image for validation
check-docker:
    docker buildx build --tag lets-chat:check -f ci-build/Dockerfile.web .

# Build desktop Linux Docker image for validation
check-desktop-linux:
    docker buildx build --tag lets-chat-desktop:check -f ci-build/Dockerfile.desktop-linux .

# Build Tailwind CSS from source
build-css:
    bun install --frozen-lockfile
    bun run tailwindcss --input assets/tailwind.css --output assets/tailwind-built.css --minify

# Build release binary
build: build-css
    cargo build --release

# Build Docker image
build-docker:
    docker buildx build --tag lets-chat:local -f ci-build/Dockerfile.web .

# Show available dev targets
dev:
    @echo "Please use a target-specific recipe: just dev-web or just dev-desktop"

# Start development server (web)
dev-web:
    dx serve --platform web

# Start development server (desktop)
dev-desktop:
    dx serve --platform desktop

# Run tests
test:
    cargo test

# Verify the server binary starts and responds to HTTP requests
verify:
    #!/usr/bin/env nu
    let server_bin = "./target/dx/lets-chat/debug/web/lets-chat"
    let log_file = "/tmp/lets-chat-verify.log"
    let pid_file = "/tmp/lets-chat-verify.pid"
    print "Building with dx..."
    dx build --platform web out+err>| lines | last 5 | each { print $in }
    print ""
    print "Starting server..."
    ^bash -c $"($server_bin) > ($log_file) 2>&1 & echo $! > ($pid_file)"
    sleep 2sec
    let server_pid = (open $pid_file | str trim | into int)
    let alive = (ps | where pid == $server_pid | length)
    if $alive == 0 {
        print "FAIL: Server process exited prematurely"
        print (open $log_file)
        exit 1
    }
    print $"Server is running \(PID ($server_pid)\), checking HTTP..."
    let http_code = (try { ^curl --silent --output /dev/null --write-out '%{http_code}' http://127.0.0.1:8080/ } catch { "000" })
    ^kill --signal TERM $server_pid
    if $http_code == "200" {
        print $"PASS: Server responded with HTTP ($http_code)"
    } else {
        print $"FAIL: Server responded with HTTP ($http_code) \(expected 200\)"
        print (open $log_file)
        exit 1
    }

# Format code
fmt:
    cargo fmt
