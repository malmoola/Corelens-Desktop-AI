# Installing Corelens Desktop AI on Windows

Corelens Desktop AI's Windows installer, and every program inside it that
its publisher had not already signed, is **Authenticode-signed** by
*Corelens-Desktop-AI-code-signing*, a certificate issued by the product's
own certificate authority, *Corelens-Desktop-AI-CA*. It is not a commercial
certificate, so Windows shows the publisher only on a machine that trusts
that root (step 2 below); elsewhere it still shows *Unknown publisher*.
What holds everywhere: the installer's checksum, the VirusTotal scan it
passed before it was published, and the minisign signature the app checks
before it installs an update of itself. This page is how a customer's
administrator checks, trusts and deploys it, and what to expect from
SmartScreen.

## What comes with each build

From a green CI run's `corelens-windows-v<version>` artifact, or the
release's assets:

| File | What it is |
| --- | --- |
| `Corelens-Desktop-AI_<version>_x64-setup.exe` | The installer. |
| `Corelens-Desktop-AI_<version>_x64-setup.exe.sig` | Its updater signature, which the app checks before installing an update. |
| `Corelens-Desktop-AI-CA.crt`, `Corelens-Desktop-AI-code-signing.crt` | The public halves of the signing chain: the root to trust, and the certificate the installer is signed with. Never a private key. |
| `SHA256SUMS.txt` | The installer's SHA-256, recorded after the VirusTotal scan passed. A release's copy lists every file it offers to download: this installer, the macOS disk image and the macOS update archive. |
| `sbom/*.cdx.json` | CycloneDX software bills of materials: the Rust executable, the collector sidecar's Python, the page's npm packages. |

No private key is ever in any of these.

## 1. Check the file is the one that was built

```powershell
(Get-FileHash .\Corelens-Desktop-AI_<version>_x64-setup.exe -Algorithm SHA256).Hash.ToLower()
Get-Content .\SHA256SUMS.txt
```

The first must equal the installer's line in the second.

## 2. Trust the root, then SmartScreen and Microsoft Defender

- **Trust the root once per machine** and Windows names the publisher, and
  `Get-AuthenticodeSignature` reports the installer **Valid**. From an
  elevated PowerShell, beside the downloaded `Corelens-Desktop-AI-CA.crt`:
  `Import-Certificate -FilePath .\Corelens-Desktop-AI-CA.crt -CertStoreLocation Cert:\LocalMachine\Root`.
  Across an estate, a GPO (*Computer Configuration → Policies → Windows
  Settings → Security Settings → Public Key Policies → Trusted Root
  Certification Authorities*) or an Intune trusted-certificate profile
  does the same. Check its thumbprint against the one in the repository's
  `brand/signing/` first. Trusting it is a choice: it is a root, and a
  machine that trusts it trusts what it signs.
- **User Account Control** shows *Unknown publisher* on a machine that does
  not trust that root.
- **Defender SmartScreen**'s "Windows protected your PC" prompt looks only
  at files that carry the *Mark of the Web* — files downloaded through a
  browser. So:
  - on one machine, clear the mark before running it:
    `Unblock-File .\Corelens-Desktop-AI_<version>_x64-setup.exe` (or
    *Properties → Unblock*), or choose *More info → Run anyway*;
  - across an estate, deploy the installer the way managed software is
    deployed — Intune, Configuration Manager, a file share, a GPO software
    installation — none of which set the mark, so SmartScreen never asks.
- **Microsoft Defender Antivirus** scans the installer like any file. Each
  build's installer has already passed VirusTotal with no detection before
  it was published (the build fails otherwise). **App Control for Business /
  WDAC** estates can allow it by a publisher rule on the signing
certificate once the root is trusted, or by a hash rule per installer.
- **Chrome may refuse the download outright** — "This file is dangerous",
  with no *Keep* button. That is Google Safe Browsing judging an installer
  with no download reputation, signed or not; it is not a detection. Three ways past it,
  from best to worst:
  1. Fetch it without a browser. From a terminal with the GitHub CLI signed
     in: `gh release download v<version> -R <owner>/<repo>`, or
     `gh run download <run id> -n corelens-windows-v<version> -R
     <owner>/<repo>`. Nothing is flagged, and the file arrives without the
     Mark of the Web.
  2. Download with Edge, which warns but offers *Keep*.
  3. In Chrome, *chrome://settings/security* → Safe Browsing → *No
     protection* for the one download, then put it back. Do not leave it off.

## 3. Install

Run the installer (per-machine; it asks for elevation). It installs the
collector sidecar — a private copy of Python with its own libraries — in
its own folder inside the installation directory. It is used only by
Corelens, is not put on `PATH`, and is removed with Corelens.

**Updating from inside the app** (Settings on the start screen → **Check for updates**)
downloads the next version's installer — the same signed `-setup.exe` that
is on the Releases page — checks its updater signature against the key
built into the app, and runs it. It asks for elevation exactly as a manual
install does, and everything in sections 1 and 2 applies to it unchanged.
Nothing is checked or downloaded unless the button is pressed or the
automatic check beside it has been switched on; it is off by default.

## Beside Corelens Desktop

Corelens Desktop AI is a separate product, not an update to Corelens
Desktop. Windows sees two different programs and installs them side by
side; each has its own data (`%LOCALAPPDATA%\Corelens Desktop AI` and
`%LOCALAPPDATA%\com.ritajit.corelens-desktop-ai`), its own vault key in
Windows Credential Manager under *Corelens Desktop AI*, and its own
updates. Corelens Desktop AI starts empty: nothing is read from, copied
out of or changed in an installed Corelens Desktop. To carry a project
across, export it from one and open it in the other.

