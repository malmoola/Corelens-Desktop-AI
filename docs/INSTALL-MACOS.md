# Installing Corelens Desktop AI on a Mac

The macOS build is a universal app (Apple Silicon and Intel) in a `.dmg`,
from a CI run's `corelens-macos-v<version>` artifact. It is **signed ad hoc,
not with an Apple Developer ID, and not notarized**, so macOS will not open
it with a double-click the first time. Nothing about that changes what the
app does; it changes only how macOS greets it.

## 1. Install

Open the `.dmg` and drag *Corelens Desktop AI* to *Applications*.

## 2. Open it the first time

Double-click it; macOS says it cannot check the app. Then open *System
Settings → Privacy & Security*, scroll to the message about Corelens
Desktop AI and choose **Open Anyway**, then **Open**. From then on it opens
normally.

## 3. The collector

The app carries the Python collector (Discover → Advanced → Engine →
*Corelens collector*) inside it, for **Apple Silicon** Macs. On an Intel
Mac that engine says it cannot run there; choose the *Classic crawler*,
which needs no Python. Everything else in the app is the same on both.

Files copied out of a downloaded `.dmg` keep macOS's quarantine mark, and
the collector's interpreter is a program of its own inside the app. If
Discover says the collector could not start, remove the mark from the
installed app once, in Terminal:

```sh
xattr -dr com.apple.quarantine "/Applications/Corelens Desktop AI.app"
```

## 4. Where it keeps things

The database and the logs are under
`~/Library/Application Support/Corelens Desktop AI/`, the window's own
storage under `~/Library/WebKit/com.ritajit.corelens-desktop-ai/`, and
saved passwords and API keys in the login keychain under *Corelens Desktop
AI*. Removing the app from *Applications* leaves all three; delete the two
folders and those keychain items to remove them too.
