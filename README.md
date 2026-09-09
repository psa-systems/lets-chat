# Let's Chat

Self-hosted team chat: rooms, DMs, calls, and an HTTP API, shipped as a single Rust binary. Talk on your own terms.

<!--
LC-873 records the Let's Chat walkthrough GIF. When it lands, commit it in-repo
at docs/assets/lets-chat-walkthrough.gif (a raw asset URL is fragile and a
cross-repo relative path does not render on the mirrors) and replace this
comment with:
![Let's Chat walkthrough](docs/assets/lets-chat-walkthrough.gif)
The committed lets-chat.png is a personal screenshot, not a product demo, and
LC-873 retires it.
-->

## Try it

Live staging: **<https://chat.a8n.systems>**. Sign in with Bunyip; a throwaway account created at <https://a8n.systems> works.

> Staging shows features **in development**, not a polished demo. It tracks the latest build from `main`, and its data can be reset or redeployed without notice - rooms, messages, and uploads are throwaway. Do not reuse a real password.

## Documentation

Everything else is in [`docs/`](docs/README.md), indexed there in full:

- [quickstart.md](docs/quickstart.md) - run the container, or bring up the local dev stack
- [configuration.md](docs/configuration.md) - every environment variable and what it gates
- [features.md](docs/features.md) - what the product does, surface by surface
- [operations.md](docs/operations.md) - health probes and boot dependencies
- [architecture.md](docs/architecture.md) - the stack, the build modes, the repository layout
- [api.md](docs/api.md) - the JSON HTTP API v1, scopes, and webhooks

Upgrading a deployment starts with [`CHANGELOG.md`](CHANGELOG.md), which calls out behavior changes, security fixes, and new variables you may need to set first.

## Development happens on Forgejo

The development home for this repository is <https://dev.a8n.run/psa-systems/lets-chat>. The [GitHub](https://github.com/psa-systems/lets-chat) and [Codeberg](https://codeberg.org/psa-systems/lets-chat) copies are read-only mirrors that exist for visibility only: issues and pull requests are disabled there, and no community support runs on the mirrors. File issues and open pull requests on Forgejo.

## Security

Please do not report a suspected vulnerability through the public issue tracker, on Forgejo or on either mirror: filing it there publishes it. Contact a maintainer privately instead. A published disclosure address and a `SECURITY.md` are being set up and this section will link to them.

## License

MIT. See [LICENSE](LICENSE).

## Authors and credits

Let's Chat is built by PSA Systems.

Built on [Rust](https://www.rust-lang.org/), [Axum](https://github.com/tokio-rs/axum), [Askama](https://github.com/djc/askama), [HTMX](https://htmx.org/), [SQLite](https://sqlite.org/) via [SQLx](https://github.com/launchbadge/sqlx), [Tailwind CSS](https://tailwindcss.com/) and [Project Fluent](https://projectfluent.org/), with the desktop app on [Tauri](https://tauri.app/) and stage audio on [LiveKit](https://livekit.io/). Driven by [just](https://github.com/casey/just) and [Nushell](https://www.nushell.sh/), and deployed behind [Traefik](https://traefik.io/).
