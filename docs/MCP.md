# Using Corelens Desktop AI from Claude

Claude Code or Claude Desktop, on the same computer as Corelens Desktop AI,
can read the project open in Corelens and run read-only commands on its
devices, through the Model Context Protocol (MCP). Corelens does the
connecting, over this computer's own network (your VPN) with the logins in
its vault; Claude gets the redacted result. What is and is not sent is in
the README, under *AI is optional, off by default*.

## 1. Install Corelens Desktop AI on Windows

Either build the installer on the laptop, or take it from a CI run.

**Build it on the laptop.** Once: [Rust](https://rustup.rs) (the MSVC
toolchain), Visual Studio 2022 Build Tools with *Desktop development with
C++*, [Node.js 22](https://nodejs.org), Git. Then, in PowerShell:

```powershell
git clone https://github.com/malmoola/Corelens-Desktop-AI.git
cd Corelens-Desktop-AI
npm ci
npm run tauri build -- --bundles nsis
```

The installer is `target\release\bundle\nsis\Corelens Desktop AI_<version>_x64-setup.exe`.
This leaves out the collector's private Python (the catalog-driven *Collect*
tab); crawling, the terminal, backups and everything MCP uses work without
it. To include it, run `pwsh sidecar/build/windows.ps1` first (needs Python
3.12) and add `--config src-tauri/tauri.sidecar.conf.json` to the build.

**Or from CI.** Actions → *build* → *Run workflow* on a branch other than
`main` builds every installer without publishing a release; the Windows one
is in the run's `corelens-windows-v<version>` artifact (`gh run download
<run id> -n corelens-windows-v<version>`). Run on `main`, or with a `v*` tag
pushed, the same run also publishes a GitHub Release.

**Install.** Run the installer (it asks for elevation). It is not
Authenticode-signed yet, so SmartScreen may say *Windows protected your PC*:
*More info → Run anyway*. It installs to
`C:\Program Files\Corelens Desktop AI\corelens-desktop-ai.exe`.

## 2. Switch it on

Open Corelens Desktop AI, open the customer's project, unlock the vault,
then *Settings → AI → Let Claude use Corelens*. The section then shows the
two snippets below with this installation's own path; copy them from there.

## 3. Connect Claude Code

In a terminal, once:

```powershell
claude mcp add --transport stdio --scope user corelens -- "C:\Program Files\Corelens Desktop AI\corelens-desktop-ai.exe" --mcp
claude mcp list
```

`claude mcp list` should show `corelens` as connected. In a Claude Code
session, `/mcp` lists its nine tools. `--scope user` makes it available in
every folder you start Claude Code in; leave it out to add it to the
current project only.

## 4. Or connect Claude Desktop

Claude Desktop → *Settings → Developer → Edit Config* opens
`%APPDATA%\Claude\claude_desktop_config.json`. Add (or merge into an
existing `mcpServers`):

```json
{
  "mcpServers": {
    "corelens": {
      "command": "C:\\Program Files\\Corelens Desktop AI\\corelens-desktop-ai.exe",
      "args": ["--mcp"]
    }
  }
}
```

Quit Claude Desktop completely and start it again. Its MCP logs are in
`%APPDATA%\Claude\logs` (`mcp.log`, `mcp-server-corelens.log`).

## 5. Five minutes on a real switch, over the VPN

Before starting: the VPN is up; the switch is a device in the open project
with its management address; an SSH login for it is saved in the vault
(assigned to the device, or one of the project's logins); the vault is
unlocked; *Let Claude use Corelens* is on.

Ask Claude, one at a time, and check each answer:

1. **"Call corelens_status."** — names the project; says the vault is
   unlocked.
2. **"List the devices."** — the switch is there, with its address and
   "login saved".
3. **"Run `show version` on SW1."** (your switch's name) — comes back within
   a few seconds, names the platform (for example *Cisco IOS / IOS-XE*), with
   the output and, where a template reads it, parsed rows.
4. **"Run `show ip interface brief` on SW1 and tell me which interfaces are
   down."** — a real answer from real output.
5. **"Run `show running-config | include username|enable|snmp-server community`
   on SW1."** — every password, secret and community appears as
   `<removed-by-corelens>`.
6. **"Run `configure terminal` on SW1."** — refused, saying only read-only
   commands can be run. Nothing was sent: the switch's `show logging` or AAA
   accounting shows no session for it.
7. **"Run `show version` on 203.0.113.99."** (any address not in the
   project) — refused: not a device of the open project.
8. **Disconnect the VPN**, then **"Run `show clock` on SW1."** — an error
   within about ten seconds saying the switch did not answer and to check
   the VPN. Reconnect.
9. In Corelens, *Settings → AI → What Claude has asked* lists every call
   above, the commands and exactly what was returned. *Export change
   record…* saves it as Markdown.
10. Switch *Let Claude use Corelens* off, and ask once more — Claude is told
    Corelens is not running or the switch is off.

If step 3 fails, the error names what to check: the VPN, the address, the
login, or a changed host key (*Settings → Host keys*).
