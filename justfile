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

# Run clippy lints (standalone server + desktop)
[group('check')]
check-clippy:
    ./dev/cargo clippy -p lets-chat-server
    ./dev/cargo-desktop clippy -p lets-chat-desktop

# Run clippy lints (saas server)
[group('check')]
check-clippy-saas:
    ./dev/cargo clippy -p lets-chat-server --no-default-features --features saas

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

# Start development server (web, standalone) via Docker with Traefik
[group('dev')]
dev-web:
    @echo "Web: https://{{ env('USER') }}-chat.a8n.run"
    BUILD_MODE=standalone GIT_HASH="$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)" GIT_VERSION="$(git describe --tags --always --dirty 2>/dev/null || echo unknown)" BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" docker compose -f compose.dev.yml up --build

# Start development server (web, saas) via Docker with Traefik
[group('dev')]
dev-web-saas:
    @echo "Web: https://{{ env('USER') }}-chat.a8n.run"
    BUILD_MODE=saas GIT_HASH="$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)" GIT_VERSION="$(git describe --tags --always --dirty 2>/dev/null || echo unknown)" BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" docker compose -f compose.dev.yml up --build

# Stop dev-web containers
[group('dev')]
dev-web-down:
    docker compose -f compose.dev.yml down

# Stop dev-web containers and remove volumes
[group('dev')]
dev-web-clean:
    docker compose -f compose.dev.yml down -v

# Start development server (web, standalone) locally on http://localhost:18080
[group('dev')]
dev-web-local: build-css
    HOST_PORT=18080 ./dev/server-up -p lets-chat-server

# Start development server (web, saas) locally on http://localhost:18080
[group('dev')]
dev-web-local-saas: build-css
    HOST_PORT=18080 ./dev/server-up -p lets-chat-server --no-default-features --features saas

# Stop the local dev server container
[group('dev')]
dev-web-local-down:
    ./dev/server-down

# Start development server (desktop)
[group('dev')]
dev-desktop:
    LETS_CHAT_SERVER_URL=http://localhost:18080 ./dev/cargo-desktop run -p lets-chat-desktop

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
