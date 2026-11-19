# Desktop update signing

The desktop app only installs updates whose signature verifies against a public key baked in at build time. The operator holds the matching private key.

## Keys

- A long-lived signing keypair, held by the operator. The public half is compiled into the desktop binary; the private half signs every release manifest.
- The key algorithm matches the desktop app's verification: today, Ed25519 over the manifest bytes. The private key is a 32-byte secret; store it in a secrets manager, not on a laptop.

Minting the keypair is a one-time event. Rotating the key is a breaking change: the new public key reaches users only through a new desktop binary that embeds it.

## Release manifest

For each desktop release the operator publishes a manifest file to the update host. The manifest lists:

- `version`: the release version.
- Per platform: the binary URL, the binary size, and a checksum.
- `signature`: the Ed25519 signature over the manifest body.

The desktop app fetches the manifest, verifies the signature, compares the version to what it is running, downloads the matching binary, verifies its checksum, and prompts the user to install.

## Minting a release

1. Build the desktop binaries per platform.
2. Write the manifest. Fill in the version and the per-platform entries.
3. Sign it: compute the Ed25519 signature over the manifest bytes using the private key, embed the signature into the manifest's `signature` field.
4. Publish the manifest and the binaries to the update host.
5. Wait for the first desktop app in the field to pick up the new version.

The `releasing.md` note in the repo tracks the current release flow and any helper scripts.

## Rotating the signing key

The in-field desktop binary trusts the baked-in public key only. A new keypair reaches users like this:

1. Mint the new keypair. Keep the old private key for now.
2. Cut a desktop binary release that embeds the new public key. Sign that release's manifest with the OLD private key so in-field clients still accept it.
3. Users install that version through the normal update flow.
4. Once your fleet is on that version, cut subsequent releases with the new private key.
5. Retire the old private key.

Skipping the overlap step means the next release will not verify for any user still on the old binary; they have to install the new binary out-of-band.

## Compromise response

If the private key is suspected compromised:

1. Pull every release manifest the compromised key signed off the update host.
2. Mint a new keypair.
3. Cut a desktop binary with the new public key.
4. Distribute the new binary out-of-band (your file share, email, whatever the operator uses). The update check will not help here because users may be running a binary whose trust is compromised.
5. Once users are on the new binary, resume the normal update flow.
