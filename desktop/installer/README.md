# Windows installer (built from Linux) - LC-131

The Windows desktop installer is built **on Linux** with NSIS, no Windows host
and no wine. This sidesteps `tauri-cli`'s host-OS gate (`cargo tauri build
--bundles msi|nsis` clap-rejects non-Linux bundle values on a Linux host), which
was the only reason a native Windows CI runner (LC-138, closed won't-fix) was
ever needed.

## Pieces

- `lets-chat.nsi` - the NSIS script. Installs the binary under Program Files,
  creates a Start-Menu shortcut, writes the uninstaller + Add/Remove Programs
  entries, checks for the WebView2 runtime (warns if missing), and offers an
  optional autostart Run key. Parameterized by `-DAPP_EXE`, `-DAPP_VERSION`,
  `-DOUT_FILE` (all have spike defaults).
- `../../ci-build/Dockerfile.desktop-windows-installer` - the packaging stage.
  A `debian:trixie-slim` image (matching the cross-build base) with the `nsis`
  package; `makensis` (the NSIS compiler) wraps the already-cross-built
  `lets-chat-desktop.exe` into `lets-chat-desktop-setup-x86_64.exe`.

`makensis` is the Linux-native NSIS *compiler*; NSIS is the system/scripting
language it compiles. They are not alternatives - you author a `.nsi` and run
`makensis` over it.

## Build (local)

```sh
# 1. Cross-build the raw binary (existing path):
docker build -f ci-build/Dockerfile.desktop-windows -t lc-win .
id=$(docker create lc-win)
mkdir -p artifacts
docker cp "$id:/build/target/x86_64-pc-windows-gnu/release/lets-chat-desktop.exe" artifacts/lets-chat-desktop.exe
docker rm "$id"

# 2. Wrap it in the installer:
docker build -f ci-build/Dockerfile.desktop-windows-installer \
  --build-arg APP_VERSION=1.2.3 -t lc-win-installer .
id=$(docker create lc-win-installer)
docker cp "$id:/out/lets-chat-desktop-setup-x86_64.exe" artifacts/
docker rm "$id"
```

## Spike result (2026-05-26)

`makensis v3.11` on Debian trixie compiled `lets-chat.nsi` (MUI2 pages,
install/uninstall sections, ARP registry, WebView2 check, optional autostart)
and the standalone Dockerfile produced a valid ~160 KB
`PE32 ... Nullsoft Installer self-extracting archive`, extractable via
`docker create` + `docker cp` exactly as the release workflow does for the raw
`.exe`. The Linux-only toolchain is confirmed viable.

## Remaining for LC-131 (full implementation)

- Wire the packaging step into `.forgejo/workflows/build-desktop-windows.yml`:
  after extracting the raw `.exe` to `artifacts/`, `docker build` the installer
  Dockerfile and upload `lets-chat-desktop-setup-x86_64.exe` to the Generic
  Packages registry alongside the raw `.exe`.
- **WebView2 bootstrap**: the script currently only *warns* when the runtime is
  absent. Decide between bundling `MicrosoftEdgeWebview2Setup.exe` (offline) or
  downloading it at install time, and run it when missing.
- **Code signing**: sign the installer (and ideally the binary) from Linux with
  `osslsigncode`. Self-signed for internal use first; OV/EV cert later.
- **Install on a real Windows 10+ box**: confirm Program Files layout, the
  Start-Menu shortcut, the ARP entry, first-run welcome page, and a clean
  uninstall. (The Linux build only proves the installer compiles + is a valid
  PE; runtime behavior needs a Windows check.)
- Optionally add a per-user (non-admin, `$LOCALAPPDATA`) install variant.
