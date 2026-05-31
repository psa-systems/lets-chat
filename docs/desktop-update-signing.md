# Desktop self-update signing (LC-210-BINARY-INTEGRITY, #277)

The desktop self-updater (`desktop/src/update.rs`) downloads a release binary and replaces the running executable in place. LC-210 SSRF-guarded the *fetch* (every redirect hop is validated against a public-IP filter), but the SSRF guard does not make the downloaded bytes *trustworthy*: a redirect to a public attacker-controlled host, a compromised package mirror, or a TLS-trust break would still serve a binary that the updater would execute. This document describes the signing layer that closes that gap, and how an operator provisions it.

## Chain of trust

1. A 32-byte Ed25519 **public key** is embedded in the desktop binary at build time (`update_verify::PUBLIC_KEY_HEX`, fed from the `LETS_CHAT_UPDATE_PUBLIC_KEY` build env). The matching **private key** is a CI release secret and never ships.
2. The release pipeline (`.forgejo/workflows/publish-release.yml`) publishes, under the fixed `latest/` pseudo-version:
   - `latest.json` - the manifest, now carrying a per-artifact lowercase-hex `sha256`.
   - `latest.json.sig` - the detached raw 64-byte Ed25519 signature over the exact bytes of `latest.json`.
3. The updater (`update::fetch_manifest`) fetches both, verifies the signature over the raw manifest bytes **before parsing the JSON**, then (`update::apply`) downloads the platform binary, hashes it, and compares against the signed `sha256` **before** writing anything or calling `self_replace`.

Because the `sha256` lives inside the signed manifest, an attacker who swaps the binary OR rewrites the hash fails verification. Both checks fail closed.

## Fail-closed default

A desktop build with no embedded key (`LETS_CHAT_UPDATE_PUBLIC_KEY` unset, the state before the first signed release) has `PUBLIC_KEY_HEX == ""`. Verification then returns `NotConfigured` and the updater refuses to apply anything:

- `--update` prints `update signing is not configured in this build; refusing to self-update` and exits non-zero.
- The GUI background check (`update::check`) silently reports no update (it already swallows fetch errors).

So an unkeyed build can never install an unverified binary; it simply has no working self-update until a key is provisioned. The CI sign step likewise refuses to publish an unsigned manifest if its private-key secret is missing.

## One-time provisioning

### 1. Generate the keypair

```nu
# Ed25519 private key (PKCS#8 PEM). Keep this secret.
^openssl genpkey -algorithm ed25519 -out ed25519_private.pem

# Derive the 32-byte raw public key as hex. An Ed25519 SPKI DER is 44 bytes
# (12-byte header + 32-byte key), so the last 32 bytes are the raw key.
let pubkey_hex = (^openssl pkey -in ed25519_private.pem -pubout -outform DER | tail -c 32 | xxd -p -c 32 | str trim)
print $pubkey_hex   # must be 64 hex characters
```

### 2. Provision CI

- **Repo/org variable `DESKTOP_UPDATE_PUBLIC_KEY`** = the 64-char `pubkey_hex`. This is NOT secret (it is a public key); a variable, not a secret, so it can be embedded into the build via `--build-arg` and is visible in build logs. The publish workflow passes it to both desktop Dockerfiles, which set it as the `LETS_CHAT_UPDATE_PUBLIC_KEY` build env so `option_env!` bakes it into the binary.
- **Repo/org secret `DESKTOP_UPDATE_SIGNING_KEY`** = the full contents of `ed25519_private.pem`. The `Sign update manifest` step writes it to a temp file, signs `latest.json` with `openssl pkeyutl -sign -rawin`, and deletes the temp file. `openssl pkeyutl -rawin` requires OpenSSL 3.x (present on the openSUSE runner).

### 3. Cut a release

`just create-release <major|minor|hotfix>` as usual (see `docs/releasing.md`). On the resulting `v*` tag, the publish workflow builds key-embedded binaries, computes per-artifact hashes, signs the manifest, and uploads `latest.json` + `latest.json.sig`.

After the first signed release, delete the local `ed25519_private.pem` (it now lives only in the CI secret store):

```nu
rm ed25519_private.pem
```

## Key rotation

The public key is embedded per build, so rotation is: generate a new keypair, update `DESKTOP_UPDATE_PUBLIC_KEY` + `DESKTOP_UPDATE_SIGNING_KEY`, and cut a new release. Clients running an OLD binary still trust only the OLD key, so they cannot update to a release signed only by the NEW key. To rotate without stranding clients, cut one release whose binaries embed the NEW public key while the manifest is still signed by the OLD key (sign with the old key for that one release), let clients update onto the new-key binaries, then switch the signing secret to the new key. A keyring (multiple accepted public keys) would remove this dance; it is intentionally out of scope here (single embedded key, documented rotation path).

## Local testing against a fixture mirror

The verification logic is unit-tested in `desktop/src/update_verify.rs` (good signature, tampered manifest, wrong key, hash mismatch, malformed inputs, fail-closed default). To exercise the full fetch+verify path against a local mirror, sign a fixture manifest with a test key, build the desktop binary with `LETS_CHAT_UPDATE_PUBLIC_KEY` set to the matching public key, and point `LETS_CHAT_UPDATE_URL` at the fixture with `LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE=1` (loopback is non-public, so the SSRF guard would otherwise refuse the initial URL).

## What this does NOT cover

- **Downgrade protection / rollback signing.** A signed-but-older manifest still verifies; `is_newer` only advertises an update when the version is strictly higher, but there is no anti-rollback floor.
- **Transparency / revocation.** No signed version log, no key revocation list. A keyring + rotation policy is the follow-up if those become requirements.
