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

# LC-512: vendor the LiveKit browser SDK same-origin (no CDN at runtime, no CSP
# change). Pinned version; the output is gitignored + produced at build time
# like tailwind-built.css. Stage audio is inert until this asset + a LiveKit
# server are present, so this is best-effort (build does not fail without net).
livekit_version := "2.5.0"
[group('build')]
vendor-js:
    mkdir -p server/assets/vendor
    curl -fsSL "https://cdn.jsdelivr.net/npm/livekit-client@{{ livekit_version }}/dist/livekit-client.umd.min.js" -o server/assets/vendor/livekit-client.umd.min.js || echo "warning: could not vendor livekit-client (stage audio will be inert)"

# Build release binary (standalone)
[group('build')]
build: build-css vendor-js
    ./dev/cargo build --release -p lets-chat-server

# Build release binary (saas)
[group('build')]
build-saas: build-css vendor-js
    ./dev/cargo build --release -p lets-chat-server --no-default-features --features saas

# Build Docker image (standalone)
[group('build')]
build-docker: build-css
    docker buildx build --tag lets-chat:local {{ docker_version_args }} -f ci-build/Dockerfile.web .

# Build Docker image (saas)
[group('build')]
build-docker-saas: build-css
    docker buildx build --tag lets-chat-saas:local --build-arg BUILD_MODE=saas {{ docker_version_args }} -f ci-build/Dockerfile.web .

# Build desktop binaries (Linux x86_64 + Windows x86_64). Outputs land in artifacts/.
[group('build')]
build-desktop: build-desktop-linux build-desktop-windows

# Mirrors the .forgejo/workflows/build-desktop-linux.yml pipeline so a local
# `just build-desktop-linux` produces the same artifact CI publishes. Builds
# via ci-build/Dockerfile.desktop-linux and copies the binary out of the build
# image to artifacts/lets-chat-desktop-linux-x86_64. Bash shebang is required
# so the $(...) substitutions inside docker_version_args expand on the host
# (nu would forward them as literal strings, ending up in the binary).
# Build the Linux x86_64 desktop binary into artifacts/.
[group('build')]
build-desktop-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    docker buildx build --tag lets-chat-desktop:local --load {{ docker_version_args }} -f ci-build/Dockerfile.desktop-linux .
    mkdir -p artifacts
    container=$(docker create lets-chat-desktop:local)
    trap 'docker rm "$container" >/dev/null 2>&1 || true' EXIT
    docker cp "$container:/build/target/release/lets-chat-desktop" artifacts/lets-chat-desktop-linux-x86_64
    echo "Artifact: artifacts/lets-chat-desktop-linux-x86_64"

# Slower than build-desktop-linux (installs tauri-cli first); keep this for
# producing distributables and use build-desktop-linux for fast iteration on
# the binary alone. Copies the binary AND both bundles into artifacts/.
# Build the Linux .deb + .AppImage bundles via the Tauri 2 CLI bundler.
[group('build')]
build-desktop-linux-bundles:
    #!/usr/bin/env bash
    set -euo pipefail
    docker buildx build --tag lets-chat-desktop:bundles --load {{ docker_version_args }} -f ci-build/Dockerfile.desktop-linux-bundles .
    mkdir -p artifacts
    container=$(docker create lets-chat-desktop:bundles)
    trap 'docker rm "$container" >/dev/null 2>&1 || true' EXIT
    docker cp "$container:/build/target/release/lets-chat-desktop" artifacts/lets-chat-desktop-linux-x86_64
    # Bundles land under /build/target/release/bundle/{deb,appimage}/ with
    # version-stamped filenames; copy each whole directory and let the user
    # inspect what's there rather than guessing exact filenames.
    rm -rf artifacts/bundle-linux
    mkdir -p artifacts/bundle-linux
    docker cp "$container:/build/target/release/bundle/deb/." artifacts/bundle-linux/
    docker cp "$container:/build/target/release/bundle/appimage/." artifacts/bundle-linux/
    echo "Artifact: artifacts/lets-chat-desktop-linux-x86_64"
    echo "Bundles in artifacts/bundle-linux/ :"
    ls -1 artifacts/bundle-linux/

# Mirrors .forgejo/workflows/build-desktop-windows.yml. Cross-builds via
# ci-build/Dockerfile.desktop-windows (mingw-w64 toolchain inside the
# rust-builder-glibc-windows image) and copies the binary out to
# artifacts/lets-chat-desktop-windows-x86_64.exe. Bash shebang for the same
# reason as build-desktop-linux.
# Cross-build the Windows x86_64 desktop binary into artifacts/.
[group('build')]
build-desktop-windows:
    #!/usr/bin/env bash
    set -euo pipefail
    docker buildx build --tag lets-chat-desktop-windows:local --load {{ docker_version_args }} -f ci-build/Dockerfile.desktop-windows .
    mkdir -p artifacts
    container=$(docker create lets-chat-desktop-windows:local)
    trap 'docker rm "$container" >/dev/null 2>&1 || true' EXIT
    docker cp "$container:/build/target/x86_64-pc-windows-gnu/release/lets-chat-desktop.exe" artifacts/lets-chat-desktop-windows-x86_64.exe
    echo "Artifact: artifacts/lets-chat-desktop-windows-x86_64.exe"

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

# Build and run the production-shape image via Docker Compose (compose.yml) on
# http://127.0.0.1:8080. Supply the required Bunyip SSO vars (and any optional
# features) with an env file: copy .env.standalone, fill it in, then run
# `just run` after adding `env_file: [.env.standalone]` to compose.yml or pass
# `--env-file .env.standalone` on the command line.
[group('run')]
run:
    {{ compose_env }} docker compose --file compose.yml up --build

# Stop the compose.yml container
[group('run')]
run-down:
    docker compose --file compose.yml down

# Start development server (web, standalone) locally on http://localhost:18080
[group('dev')]
dev-web-local: build-css
    {{ compose_uid }} {{ compose_env }} docker compose --file compose.dev-web-local.yml up

# Start the local dev server with a mock OIDC OP (no bunyip needed). DEV ONLY:
# boots the server for unauthenticated debug routes (e.g. /dev/theme-gallery).
# Authed pages still need the real bunyip dev-sso stack. See dev/mock-oidc.py.
[group('dev')]
dev-web-local-mock: build-css
    {{ compose_uid }} {{ compose_env }} docker compose --file compose.dev-web-local.yml --file compose.dev-web-local-mock-sso.yml up

# Stop the mock-OIDC local dev server (both overlay containers)
[group('dev')]
dev-web-local-mock-down:
    docker compose --file compose.dev-web-local.yml --file compose.dev-web-local-mock-sso.yml down

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
    #!/usr/bin/env bash
    set -euo pipefail
    # Default XAUTHORITY to the canonical location so compose has a real
    # host path to bind-mount the X11 cookie file from. Touching is a no-op
    # when the file exists; on a fresh login the file is created empty so
    # the mount works, with xhost (below) covering the auth side instead.
    : "${XAUTHORITY:=$HOME/.Xauthority}"
    export XAUTHORITY
    touch "$XAUTHORITY"
    # Grant the local user access to the running X server (when there is
    # one) so the container's GTK can connect without needing to read the
    # cookie file. Silently no-op on Wayland-only sessions or hosts without
    # the xhost utility.
    if [ -n "${DISPLAY:-}" ] && command -v xhost >/dev/null 2>&1; then
        xhost +SI:localuser:"$(id -un)" >/dev/null 2>&1 || true
    fi
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

# Run desktop crate tests (LC-210 established the pattern: #[cfg(test)] modules
# in desktop/src/). Desktop is bin-only, so these are in-crate unit tests. Run
# this for any PR touching desktop/ - `just check` only compiles it.
[group('test')]
test-desktop:
    ./dev/cargo-desktop test -p lets-chat-desktop

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

# ── Cleanup ────────────────────────────────────────────────────────────────

# Tear down THIS repo's entire dev footprint: bring down every dev compose stack (web-local, web-local-saas, desktop) plus the compose.yml run stack with their networks and project-scoped volumes, remove the fixed-name cargo/data volumes the `dev/cargo` + `dev/server-up` wrappers create, and delete local build/runtime artifacts (target/, data/, artifacts/ desktop bundles). Supersedes the per-mode dev-web*-clean recipes (each only `down --volumes` for one stack); this one teardown covers every mode at once. Scoped to this repo, safe on a shared host (no host-global prune).
[group: 'cleanup']
dev-clean:
    #!/usr/bin/env nu
    # Each dev mode has its own compose file; `down --remove-orphans --volumes`
    # drops the stack, its default network, and its project-scoped named
    # volumes (cargo-registry/cargo-git/target/data, lets-chat-data) without
    # needing to spell the project-prefixed volume names out by hand.
    let compose_files = [
        "compose.dev-web-local.yml"
        "compose.dev-web-local-saas.yml"
        "compose.dev-desktop.yml"
        "compose.yml"
    ]
    for f in $compose_files {
        if ($f | path exists) {
            docker compose --file $f down --remove-orphans --volumes
        }
    }
    # Fixed-name volumes from the ad-hoc `docker run` wrappers (dev/cargo,
    # dev/server-up). These are NOT compose-managed, so `compose down` above
    # never touches them; remove by name, guarded so a missing one is a no-op.
    let vols = [
        "lets-chat-rewrite-cargo-registry"
        "lets-chat-rewrite-cargo-git"
        "lets-chat-rewrite-target"
        "lets-chat-rewrite-data"
    ]
    let existing = docker volume ls --quiet | lines
    for vol in $vols {
        if $vol in $existing {
            docker volume rm $vol
        }
    }
    # Local build + runtime artifacts: Rust target/, the dev SQLite data/ dir
    # (gitignored bind-mount state), and artifacts/ where the desktop build
    # recipes drop binaries and the deb/appimage bundles (artifacts/bundle-linux/).
    let paths = [target data artifacts]
    for p in $paths {
        if ($p | path exists) {
            rm --recursive $p
            print $"removed ($p)"
        }
    }
    print "dev-clean: done"

# Everything dev-clean does, plus remove the Docker images this repo builds locally (web + saas + desktop check/local/bundles tags) and prune the buildx cache. Run for a from-scratch rebuild.
[group: 'cleanup']
dev-clean-all: dev-clean
    #!/usr/bin/env nu
    let images = [
        "lets-chat:local"
        "lets-chat:check"
        "lets-chat-saas:local"
        "lets-chat-saas:check"
        "lets-chat-desktop:local"
        "lets-chat-desktop:bundles"
        "lets-chat-desktop-windows:local"
    ]
    for img in $images {
        let present = (do { ^docker image inspect $img } | complete).exit_code == 0
        if $present {
            docker image rm $img
        }
    }
    docker buildx prune --force
    print "dev-clean-all: done"

# ── Release ──────────────────────────────────────────────────────────────

# Create a release: bump major (vx.0.0), minor (v0.x.0), or hotfix (v0.0.x), push branch, and open the release PR via fj.
# After the PR is merged, the create-release workflow creates the tag and release automatically.
[group: 'release']
create-release bump:
    #!/usr/bin/env nu
    let bump = "{{ bump }}"

    # Abort if there are uncommitted changes
    let status = git status --porcelain | str trim
    if ($status | is-not-empty) {
        print $"(ansi red)Working tree is dirty. Please stash or commit your changes first.(ansi reset)"
        exit 1
    }

    # Switch to main if not already there
    let branch = git branch --show-current | str trim
    if $branch != "main" {
        print $"Switching from ($branch) to main..."
        git checkout main
    }

    # Pull latest changes
    git pull --rebase origin main

    # Calculate next version
    let current = (open Cargo.toml | get workspace.package.version | split row "." | each { into int })
    let next = match $bump {
        "major" => [$"($current.0 + 1)" "0" "0"],
        "minor" => [$"($current.0)" $"($current.1 + 1)" "0"],
        "hotfix" => [$"($current.0)" $"($current.1)" $"($current.2 + 1)"],
        _ => { print $"(ansi red)Usage: just create-release <major|minor|hotfix>(ansi reset)"; exit 1 }
    }
    let bare = ($next | str join ".")
    let tag = $"v($bare)"
    let release_branch = $"release/($tag)"

    # Create release branch, bump version, and commit
    git checkout -b $release_branch
    open Cargo.toml | update workspace.package.version $bare | to toml | collect | save --force Cargo.toml
    git add Cargo.toml
    git commit --signoff --message $"Release ($tag)"

    # Push release branch
    git push --set-upstream origin $release_branch

    # Open the release PR via fj. Body lives in a tempfile so the
    # changelog can grow later without inline escaping pain.
    let body_file = (mktemp --tmpdir --suffix .md)
    [
        $"Automated release PR for ($tag)."
        ""
        $"After merge, `.forgejo/workflows/create-release.yml` tags and publishes ($tag) to the Generic Packages registry."
    ] | str join "\n" | save --force $body_file
    let fj_result = (^fj --host dev.a8n.run pr create $"Release ($tag)" --body-file $body_file | complete)
    rm $body_file
    if $fj_result.exit_code != 0 {
        print $"(ansi red)fj pr create failed(ansi reset)"
        print $fj_result.stderr
        exit 1
    }

    # `fj pr create` prints `created pull request #N: <title>` on success.
    # Parse the number out and build the PR URL from `origin`.
    let pr_num = (
        $fj_result.stdout
        | str trim
        | parse --regex 'created pull request #(?P<num>\d+)'
        | get num.0?
    )
    let remote = (git remote get-url origin | str trim)
    let base_url = if ($remote | str starts-with "ssh://") {
        $remote | str replace "ssh://git@" "https://" | str replace "git.a8n.run" "dev.a8n.run" | str replace ".git" ""
    } else {
        $remote | str replace --regex "git@([^:]+):" "https://$1/" | str replace "git.a8n.run" "dev.a8n.run" | str replace ".git" ""
    }
    print $"(ansi green)Pushed ($release_branch)(ansi reset)"
    if ($pr_num | is-not-empty) {
        print $"PR: ($base_url)/pulls/($pr_num)"
    } else {
        # fj output format drifted; fall back to whatever it said.
        print $"fj output: ($fj_result.stdout | str trim)"
    }
    print $"After merging, the create-release workflow will tag and release ($tag) automatically."
