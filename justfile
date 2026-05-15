# List available recipes
default:
    @just --list

# Build args for Docker image builds: inject git metadata so the binary can
# report its exact version. .git is excluded from the Docker context, so the
# args must be computed on the host and forwarded.
docker_version_args := '--build-arg GIT_HASH="$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)" --build-arg GIT_VERSION="$(git describe --tags --always --dirty 2>/dev/null || echo unknown)" --build-arg BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"'

# Install the git pre-commit hook (run once per fresh clone). Writes a stub at .git/hooks/pre-commit that execs `just pre-commit`. Bypass with `git commit --no-verify`.
[group('setup')]
install-hooks:
    #!/usr/bin/env nu
    let hook = ".git/hooks/pre-commit"
    # Remove first so a leftover symlink from an older install does not get
    # written through to its target file. `try` swallows the not-found case.
    try { rm $hook }
    "#!/usr/bin/env sh\nexec just pre-commit\n" | save $hook
    ^chmod +x $hook
    print $"Wrote ($hook) -> just pre-commit"

# Run the workspace checks + tests via the existing `./dev/cargo` Docker wrapper. Aggregates `just check` (fmt + standalone/saas server checks + desktop + clippy) and `just test` (server tests, standalone), so a green run mirrors the in-repo `dev/cargo`-based check pipeline.
[group('check')]
pre-commit: check test

# Run all checks
[group('check')]
check: check-server check-server-saas check-desktop check-clippy check-clippy-saas check-fmt

# Check server compilation (standalone)
[group('check')]
check-server:
    ./dev/cargo check -p lets-chat-server

# Check server compilation (saas)
[group('check')]
check-server-saas:
    ./dev/cargo check -p lets-chat-server --no-default-features --features saas

# Check desktop compilation
[group('check')]
check-desktop:
    ./dev/cargo-desktop check -p lets-chat-desktop

# Run clippy lints (standalone server + desktop).
# `-D warnings` matches the CI runner so any new lint that the Rust 1.94
# clippy promotes to a warning fails the local check too, instead of slipping
# past `just check` and only blowing up after a push.
[group('check')]
check-clippy:
    ./dev/cargo clippy -p lets-chat-server --all-targets -- -D warnings
    ./dev/cargo-desktop clippy -p lets-chat-desktop -- -D warnings

# Run clippy lints (saas server)
[group('check')]
check-clippy-saas:
    ./dev/cargo clippy -p lets-chat-server --no-default-features --features saas --all-targets -- -D warnings

# Check formatting
[group('check')]
check-fmt:
    ./dev/cargo fmt --check

# Build Docker image for validation (standalone)
[group('check')]
check-docker:
    docker buildx build --tag lets-chat:check {{ docker_version_args }} -f ci-build/Dockerfile.web .

# Build Docker image for validation (saas)
[group('check')]
check-docker-saas:
    docker buildx build --tag lets-chat-saas:check --build-arg BUILD_MODE=saas {{ docker_version_args }} -f ci-build/Dockerfile.web .

# Build Tailwind CSS from source
[group('build')]
build-css:
    cd server && ../dev/bun install --frozen-lockfile
    cd server && ../dev/bun run tailwindcss --input assets/tailwind.css --output assets/tailwind-built.css --minify

# Build release binary (standalone)
[group('build')]
build: build-css
    ./dev/cargo build --release -p lets-chat-server

# Build release binary (saas)
[group('build')]
build-saas: build-css
    ./dev/cargo build --release -p lets-chat-server --no-default-features --features saas

# Build Docker image (standalone)
[group('build')]
build-docker: build-css
    docker buildx build --tag lets-chat:local {{ docker_version_args }} -f ci-build/Dockerfile.web .

# Build Docker image (saas)
[group('build')]
build-docker-saas: build-css
    docker buildx build --tag lets-chat-saas:local --build-arg BUILD_MODE=saas {{ docker_version_args }} -f ci-build/Dockerfile.web .

# Build args common to every compose recipe so the server logs the right
# git metadata in its banner. Computed on the host because the builder
# image has no git history of its repo to introspect.
compose_env := 'GIT_HASH="$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)" GIT_VERSION="$(git describe --tags --always --dirty 2>/dev/null || echo unknown)" BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"'

# HOST_UID / HOST_GID for compose recipes that run `cargo run` from source
# (dev-web-local{,-saas}, dev-desktop). The container starts as root long
# enough to chown the named volumes, then drops to the host user via
# setpriv so files written into the /work bind mount land owned by the
# developer on the host.
compose_uid := 'HOST_UID="$(id -u)" HOST_GID="$(id -g)"'

# Start development server (web, standalone) via Docker Compose with Traefik
[group('dev')]
dev-web:
    @echo "Web: https://{{ env('USER') }}-chat.a8n.run"
    {{ compose_env }} docker compose --file compose.dev-web.yml up --build

# Stop dev-web container
[group('dev')]
dev-web-down:
    docker compose --file compose.dev-web.yml down

# Stop dev-web container and remove the data volume
[group('dev')]
dev-web-clean:
    docker compose --file compose.dev-web.yml down --volumes

# Start development server (web, saas) via Docker Compose with Traefik
[group('dev')]
dev-web-saas:
    @echo "Web: https://{{ env('USER') }}-chat.a8n.run"
    {{ compose_env }} docker compose --file compose.dev-web-saas.yml up --build

# Stop dev-web-saas container
[group('dev')]
dev-web-saas-down:
    docker compose --file compose.dev-web-saas.yml down

# Stop dev-web-saas container and remove the data volume
[group('dev')]
dev-web-saas-clean:
    docker compose --file compose.dev-web-saas.yml down --volumes

# Start development server (web, standalone) locally on http://localhost:18080
[group('dev')]
dev-web-local: build-css
    {{ compose_uid }} {{ compose_env }} docker compose --file compose.dev-web-local.yml up

# Stop the local dev server container
[group('dev')]
dev-web-local-down:
    docker compose --file compose.dev-web-local.yml down

# Stop the local dev server container and remove cargo + data volumes
[group('dev')]
dev-web-local-clean:
    docker compose --file compose.dev-web-local.yml down --volumes

# Start development server (web, saas) locally on http://localhost:18080
[group('dev')]
dev-web-local-saas: build-css
    {{ compose_uid }} {{ compose_env }} docker compose --file compose.dev-web-local-saas.yml up

# Stop the local saas dev server container
[group('dev')]
dev-web-local-saas-down:
    docker compose --file compose.dev-web-local-saas.yml down

# Stop the local saas dev server container and remove cargo + data volumes
[group('dev')]
dev-web-local-saas-clean:
    docker compose --file compose.dev-web-local-saas.yml down --volumes

# Start development server (desktop)
[group('dev')]
dev-desktop:
    {{ compose_uid }} docker compose --file compose.dev-desktop.yml up

# Stop the desktop dev container
[group('dev')]
dev-desktop-down:
    docker compose --file compose.dev-desktop.yml down

# Run tests (server, standalone)
[group('test')]
test:
    # --jobs 2 caps parallel linking: each of the ~50 test binaries statically
    # links the full dep graph, and 8-way parallel `ld` exhausts memory on a
    # swapless host, getting the linker OOM-killed (SIGTERM).
    ./dev/cargo test -p lets-chat-server --jobs 2

# Run tests (server, saas)
[group('test')]
test-saas:
    ./dev/cargo test -p lets-chat-server --no-default-features --features saas

# Verify the standalone server binary starts and serves the login page
[group('test')]
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
[group('format')]
fmt:
    ./dev/cargo fmt --all
