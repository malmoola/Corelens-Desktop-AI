# Signing

Two different signatures are involved, and only one of them exists today.

| | What it proves | State |
| --- | --- | --- |
| **Updater signature** (minisign) | An update the app downloads was built by this repository's release job | **On.** Every release carries one; the app refuses an update without it |
| **Authenticode** (Windows code signing) | Who published the installer, shown by UAC and checked by SmartScreen | **Off.** This product has no certificate of its own yet |

## The updater signature

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | the minisign private key `tauri signer generate` wrote |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password, empty for a key made without one |

With the key set, `tauri build` writes a `.sig` beside the Windows installer
and beside the macOS update archive (`tauri.updater.conf.json` switches
that on), and the `release` job attaches them with `latest.json`. The
matching public key is `plugins.updater.pubkey` in `tauri.conf.json`; an
installed copy refuses any update whose signature does not verify against
it. Losing the private key means every installed copy stops being able to
update itself and has to be reinstalled by hand, so it is kept off the
repository and backed up.

## Authenticode, when there is a certificate

Nothing has to change in the build to turn it on. Two repository secrets,
at **Settings → Secrets and variables → Actions**:

| Secret | Value |
| --- | --- |
| `WINDOWS_CERTIFICATE` | `base64 -w0 <certificate>.pfx` — the PFX, exported with its issuing root inside it (`-certfile`) |
| `WINDOWS_CERTIFICATE_PASSWORD` | the PFX password |

and the **public** halves of the chain — the root and the signing
certificate, never a private key — committed under `brand/signing/` as
`.crt` files. The bundle job copies whatever is there beside the installer,
and fails if the secret's certificate is not one of them, so the notes
beside an installer can never describe a different signer.
`scripts/signing.test.mjs` checks that the signing certificate is marked for
code signing, is issued by a root published beside it, and has not expired.

`.github/actions/sign-windows` imports the PFX into the runner's certificate
store, deletes the file, and points the Tauri bundler at the imported key by
thumbprint, read back from the certificate rather than kept here.
Signatures are timestamped against `http://timestamp.digicert.com`: without
a timestamp a signature stops validating the day the certificate expires,
and an installer outlives that. The installer is signed, and so is every PE
inside the collector sidecar that its publisher has not already signed
(`sidecar/build/sign.ps1`).

## With no certificate

The default today. The signing action and `sign.ps1` each post a notice
that the build is **unsigned** and carry on; nothing fails. Each installer
is still scanned on VirusTotal and fails the build on any detection, its
SHA-256 goes into `SHA256SUMS.txt`, and CycloneDX SBOMs into `sbom/`. What
that means for whoever installs it is in `docs/INSTALL-WINDOWS.md`.

## Checking a build

On Windows:

```powershell
Get-AuthenticodeSignature .\Corelens-Desktop-AI_<version>_x64-setup.exe | Format-List
```

`NotSigned` today. Once a certificate is configured, `Valid` on a machine
that trusts its root, and `UnknownError` / `NotTrusted` on one that does
not.
