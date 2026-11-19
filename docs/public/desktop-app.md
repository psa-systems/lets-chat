# Desktop app

A native-shell wrapper around the same web UI, published per platform. The desktop app does not replace the web app; both talk to the same server and show the same rooms.

## When to prefer the desktop app

- Running the chat alongside other desktop windows (always-on-top call bar, notifications).
- Screen share into a call. The desktop paths have fewer portal and permission edge cases than the browser.
- Consent-gated remote control during a 1:1 call. Available on Windows only, off by default, and only after an admin enables it at `/admin/remote-control`.

## When the web app is fine

Occasional access, shared workstations, a phone browser, or any machine the operator does not want to install an installer on.

## Install

Operators distribute one installer per platform from their own download host. The installer paths once installed:

- Windows: `C:\Program Files\Lets Chat\lets-chat.exe`
- macOS: `/Applications/Lets Chat.app`
- Linux `.deb`: `/usr/bin/lets-chat`
- Linux raw binary: wherever you drop it
- Linux `.AppImage`: wherever you drop it

On Linux screen share requires PipeWire and `xdg-desktop-portal` with a matching portal backend. The `.deb` carries those dependencies; the raw binary and `.AppImage` do not. See the Linux prerequisites in [Installation](installation.md).

## Configure which server to connect to

On first run the desktop app asks which server to point at. The operator distributes the server URL alongside the installer. Once set the choice sticks; it can be changed from the desktop app's About screen.

The URL must be the public base URL of the server, matching `LETS_CHAT_PUBLIC_URL`. A mismatched URL leads to the sign-in loop described in [Troubleshooting](troubleshooting.md).

## Sign-in

Sign-in opens the Bunyip OP in the desktop app's shell and returns to the room list once Bunyip approves. Staying signed in is a per-install option; the refresh credential sits in the OS keyring (Credential Manager on Windows, Keychain on macOS, Secret Service on Linux).

## Update flow

The desktop app checks for a newer release at startup and once per day. The update check hits a signed release manifest on the operator's update host.

- The manifest lists each platform's binary, a version, and a signature.
- The desktop app refuses to install an update whose signature does not verify against a public key baked into the binary at build time.
- An update with a verified signature downloads to a temp directory, verifies, and prompts the user to relaunch.

An update that fails signature verification is a hard stop. The desktop app does not fall through to an unsigned download. See [operations/desktop-update-signing.md](operations/desktop-update-signing.md) for the operator side.

## Permissions

On first use the desktop app asks for microphone, camera, screen, and notification permissions. Declining breaks the feature that asked, not the app itself. The permission is a platform-level decision; the operator can reset it through system settings.

## Shortcuts

- `Ctrl+K` / `Cmd+K`: quick-switch rooms
- `Ctrl+/` / `Cmd+/`: list keyboard shortcuts
- `Ctrl+Shift+M` / `Cmd+Shift+M`: mute microphone during a call
- `Esc`: dismiss the current modal, drop out of a call, close a composer

A full shortcut list lives at `/settings/shortcuts`.
