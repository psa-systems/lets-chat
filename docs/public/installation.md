# Installation

Three deployment paths: container, bare binary, or dev stack. Choose one per environment.

## Container (recommended)

The published image carries the server, the static assets, and the SQLite runtime. One `docker run` or a compose file is enough.

```
docker run -d \
  --name lets-chat \
  -p 443:443 \
  -v /srv/lets-chat/data:/data \
  --env-file ./lets-chat.env \
  dev.a8n.run/psa-systems-private/lets-chat:latest
```

The image follows the fleet's `latest` and `vX.Y.Z` tag convention. Pin an exact version in production and roll the tag forward during a release window.

Volume layout under `/data`:
- `auth.db` - sessions and SSO state
- `chat.db` - messages, rooms, enclaves, memberships
- `files.db` - file-upload metadata
- `uploads/` - user-uploaded files
- `avatars/` - user avatars

## Bare binary

The release artifact is a single Linux x86_64 binary. Drop it behind a reverse proxy that terminates TLS, give it a writable data directory, and run it as a service. The same four `LETS_CHAT_BUNYIP_SSO_*` variables apply.

Systemd unit sketch:

```
[Service]
EnvironmentFile=/etc/lets-chat/env
WorkingDirectory=/var/lib/lets-chat
ExecStart=/usr/local/bin/lets-chat
Restart=on-failure
User=lets-chat
```

Boot requires that the data directory already exists and is writable. The server does not create it for you.

## Desktop app

The desktop app is a thin shell around the same web UI. Each platform has an installer:

- Windows: `.msi`
- macOS: `.pkg`
- Linux: `.deb`, raw binary, or `.AppImage`

Install flow is covered in [Desktop app](desktop-app.md). The desktop binary ships with an update check that fetches a signed release manifest from the operator's own update host.

### Linux screen sharing prerequisites

Screen share on Linux goes through WebKitGTK and PipeWire. The `.deb` carries the dependencies; the raw binary and `.AppImage` do not. Install the pipewire and portal packages for your distribution before the first screen share.

Debian or Ubuntu: `gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`, `gstreamer1.0-pipewire`, `pipewire`, `xdg-desktop-portal`, and one of `xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde`, `xdg-desktop-portal-gtk`, or `xdg-desktop-portal-wlr`.

Fedora: `gstreamer1-plugins-base`, `gstreamer1-plugins-good`, `pipewire-gstreamer`, `pipewire`, `xdg-desktop-portal`, plus the matching portal backend.

Arch: `gst-plugins-base`, `gst-plugins-good`, `gst-plugin-pipewire`, `pipewire`, `xdg-desktop-portal`, plus the matching portal backend.

Without them the call UI reports a screen-share failure.

## Web deployment (optional)

If you serve the web app on a different host than the API (for example a static CDN and a separate chat.example.com for the API), set `LETS_CHAT_BUNYIP_SSO_REDIRECT_URI` on the API host and point the web app at the API URL. Most deployments put both on one host and this split is unnecessary.

## Reverse proxy notes

The server speaks HTTP/1.1 and HTTP/2 natively. If you front it with a reverse proxy, pass through:

- `/` and `/ws` for the chat UI and WebSocket
- `/healthz` for the liveness probe
- `/v1/` for the HTTP API
- `/auth/bunyip/callback` for the OIDC callback

Trust only the proxies you own: `X-Forwarded-For` and `X-Forwarded-Proto` are read only when the peer is in the configured trusted-proxy range.
