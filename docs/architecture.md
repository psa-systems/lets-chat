# Architecture

A single Rust binary serving HTTP, WebSocket, and static assets, with an
optional desktop wrapper around the same pages. The workspace has two members,
`server/` and `desktop/`.

## Stack

- **Frontend**: server-rendered HTML via [Askama](https://github.com/djc/askama) templates + [HTMX](https://htmx.org/) for interactivity
- **Backend**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **Database**: SQLite via SQLx, in three separate pools (auth, chat, settings), all under `LETS_CHAT_DATA_DIR`
- **Real-time**: WebSocket hub broadcasting pre-rendered HTML fragments via `hx-swap-oob`
- **Styles**: Tailwind CSS (compiled via Bun), with semantic design tokens driving light/dark/high-contrast themes (see [ui-conventions.md](ui-conventions.md))
- **i18n**: [Project Fluent](https://projectfluent.org/) catalogs embedded at compile time (see [i18n.md](i18n.md))
- **Desktop**: optional [Tauri](https://tauri.app/) 2 wrapper in `desktop/`, over the system webview (WebKit2GTK / WebView2 / WKWebView), with a self-updater that pulls its release artifact from an OCI registry
- **Media**: WebRTC for 1:1 calls and small voice channels; a self-hosted [LiveKit](https://livekit.io/) SFU for stage audio, where the mesh does not scale

## Build modes

`server/` has two mutually exclusive Cargo features. `standalone` (the default)
is the self-hosted product: Bunyip SSO sign-in and the full admin surface.
`saas` builds the variant that delegates identity to a parent application.
`ci-build/Dockerfile.web` selects between them with `--build-arg BUILD_MODE`,
and `just check` compiles and lints both.

## Repository layout

| Path | What lives there |
|---|---|
| `server/` | the server binary: routes, templates, assets, Fluent locales, migrations |
| `desktop/` | the Tauri 2 desktop wrapper and its self-updater |
| `services/transcription-agent/` | the LiveKit transcription agent sidecar (TypeScript), published as its own image |
| `dev/` | containerized `cargo` / `bun` wrappers and the mock OIDC provider |
| `ci-build/` | Dockerfiles and the Nushell convention guards `just check` runs |
| `common/` | the shared `psa-systems/common` submodule (hook, release recipes) |
| `docs/` | this documentation |
