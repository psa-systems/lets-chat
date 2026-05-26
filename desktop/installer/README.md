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

## Done (LC-131)

- **NSIS-from-Linux toolchain** proven: `makensis v3.11` on Debian trixie
  compiles `lets-chat.nsi` (MUI2 pages, install/uninstall, ARP registry,
  WebView2 handling, optional autostart) and the Dockerfile emits a valid
  `PE32 ... Nullsoft Installer self-extracting archive`, extractable via
  `docker create` + `docker cp` exactly as the release workflow handles the raw
  `.exe`.
- **WebView2 bootstrap**: the Dockerfile bundles the Evergreen *bootstrapper*
  (passed via `-DWEBVIEW2_BOOTSTRAPPER`); when the runtime is absent at install
  time the `.nsi` runs it `/silent /install`. The installer is ~1.75 MB with it
  bundled. Without the define (the standalone spike build) the script falls back
  to a warning.
- **CI wiring**: `.forgejo/workflows/build-desktop-windows.yml` builds the
  installer right after the raw exe and publishes
  `lets-chat-desktop-setup-windows-x86_64.exe` to the Generic Packages registry
  alongside the raw binary.

## Remaining (ops-gated, not code)

- **Code signing**: sign the installer (and ideally the binary) from Linux with
  `osslsigncode` once a cert is provisioned - a cert/cost decision (self-signed
  still trips SmartScreen, so it adds little). Inject after the `makensis` RUN
  in the installer Dockerfile: `osslsigncode sign -pkcs12 cert.p12 -pass ... -in
  setup.exe -out signed.exe`, with the cert as a Docker build secret.
- **Real Windows 10+ install test** (cannot run from Linux CI): confirm the
  Program Files layout, Start-Menu shortcut, ARP entry, the WebView2 bootstrap
  firing on a clean VM, first-run welcome page, and a clean uninstall. The Linux
  build only proves the installer compiles to a valid PE.
- Optionally add a per-user (non-admin, `$LOCALAPPDATA`) install variant.
