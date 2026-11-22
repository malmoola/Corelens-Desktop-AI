# Using Corelens Desktop AI from Claude

Claude Code or Claude Desktop, on the same computer as Corelens Desktop AI,
can read the project open in Corelens, run read-only commands on its
devices, and propose configuration changes that wait for your approval in
the Corelens window, through the Model Context Protocol (MCP). Corelens does
the connecting, over this computer's own network (your VPN) with the logins
in its vault; Claude gets the redacted result. What is and is not sent is in
the README, under *AI is optional, off by default*. The same tools also
serve the chat inside Corelens (`docs/AI-CHAT.md`).

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
session, `/mcp` lists its fourteen tools. `--scope user` makes it available in
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

## 6. When a call goes wrong: the debug log

Set `CORELENS_MCP_DEBUG=1` for the `--mcp` process and each tool call
writes one line to its standard error: the tool, the keys of the request's
`params` and the keys of its `arguments`. Never a value, so no device name,
command or anything else typed reaches the log. Claude Desktop keeps that
stream in `%APPDATA%\Claude\logs\mcp-server-corelens.log`; add an `env` to
its entry and restart it:

```json
{
  "mcpServers": {
    "corelens": {
      "command": "C:\\Program Files\\Corelens Desktop AI\\corelens-desktop-ai.exe",
      "args": ["--mcp"],
      "env": { "CORELENS_MCP_DEBUG": "1" }
    }
  }
}
```

For Claude Code: `claude mcp add --transport stdio --scope user -e CORELENS_MCP_DEBUG=1 corelens -- "C:\Program Files\Corelens Desktop AI\corelens-desktop-ai.exe" --mcp`
(after `claude mcp remove corelens`).

A line looks like
`corelens-mcp: tools/call run_show_command: params keys: _meta, arguments, name; argument keys: command, target`.
An argument error says the same without the flag: it names the tool and
lists the keys it received.

**Why the device argument is called `target`.** The tools that act on one
device take it as `target`: its name or management IP, as `list_devices`
shows it. An earlier version called it `device`, and from Claude Desktop
that key never arrived — the call reached Corelens with `command` alone,
removed on the way, most likely by a bridge in the client that keeps the name for
itself. So no tool takes an argument named `device`, `computer`, `host`,
`path`, `file`, `url`, `session` or `id`, and a test fails if one ever
does. A call without `target` is answered
`run_show_command: missing 'target' (the device name or IP). Keys received: command.`

## 7. Changes: proposals that wait for you

`propose_config_change` takes a device (`target`) and the exact
configuration-mode lines (`commands`), or several devices in `batch`, with
a `reason` and, optionally, `verify_commands`. Corelens logs in, reads the
running configuration and opens the approval dialog in its window: the
commands, a diff against what is running, the risk, the rollback the
platform offers, and for lines that can cut the device off, a field for the
device's name. **The tool call waits for your decision.** Approve, and
Corelens snapshots the configuration, pushes with the platform's own
rollback ready, checks every line took effect, runs the verification
commands and reports; deny, or let 10 minutes pass (*Settings → AI*), and
nothing is sent. Saving is `propose_save_config`, a separate approval.
`get_change_status` reports a change's state and outcome.

Nothing Claude sends, and no Claude Code permission mode, approves a
change: the only approval is the click in the Corelens dialog.

**Waiting.** Claude Code waits for a tool for as long as it takes (its
default limit is about 28 hours; the idle limit for a server on stdio is 30
minutes, and Corelens sends a progress notification every 20 seconds while
a proposal waits). Claude Desktop is reported to stop waiting for a tool
after about a minute. If it does, the proposal stays open in Corelens —
approve or deny it there as usual — and ask Claude to call
`get_change_status` with the change id it was given (also shown in the
dialog's change record): that call waits up to 50 seconds for the decision
and reports what happened.

**A test on the bench** (a lab switch, never a customer's):

1. "Propose setting the description `LAB-PORT` on `Gi1/0/24` on SW1, and
   verify it with `show interfaces status`." — the dialog shows the two
   lines, the diff (`- description …`, `+ description LAB-PORT`), *Low
   risk*. Approve. Claude reports *Verified* and *Not saved*.
2. In the dialog (or *Settings → AI → Configuration changes*), **Roll
   back** — the description is as it was; `show running-config interface
   Gi1/0/24` agrees.
3. "Propose `reload` on SW1." — the dialog marks it a hard block and asks
   for the device's name. Deny.
4. "Save SW1's configuration." — a separate dialog, showing what saving
   writes over. Approve or deny.

The tool list grew in this version: restart Claude Desktop (or `/mcp` →
reconnect in Claude Code) so it reads the new list.

**If the client offers no proposal tools at all,** it is starting a
`corelens-desktop-ai.exe --mcp` from another install or build — the tool
list comes from that executable, not from the window. `corelens_status`
then ends with a warning saying so, and names the window's own build
(`tool list …`). Copy the setup command from *Settings → AI* again (it
names the executable of the Corelens you are running), run it, and restart
the client: `claude mcp remove corelens` first if `add` says it exists.


## 8. Diagnose: "check this network, what's going on"

`run_diagnostic` runs Corelens's own read-only sweep of the open project,
one site, named devices, or the path between two endpoints (`scope`
`project`, `site`, `devices` or `path`; `depth` `quick` or `full`; an
optional `symptom` in the engineer's words). It returns the report: a short
summary, root causes ranked with their evidence, findings by severity, what
passed, what could not be checked and why, what changed since the last run,
and a fix plan. The plan's steps are proposed by Corelens itself and wait in
the window for the engineer, one approval each — the model does not need to
propose them again, and cannot approve them. `get_diagnosis` returns a
report run earlier (the newest unless named), with where each step's
proposal stands now.

The call takes from seconds to many minutes on a large project; while it
runs, the Corelens window shows each device's progress and can stop it.
Claude Code waits as long as it takes; a client that gives up sooner can
read the result with `get_diagnosis` once it is done. `docs/DIAGNOSE.md`
describes what is checked.

## 9. The engineer's documents and runbooks

`list_documents`, `search_library` and `read_document` read the engineer's
own documents on this computer. They come from the open project's
documents folder and the library folder, both set in *Settings → AI →
Documents*: designs, IP plans, MOPs, runbooks and vendor guides in Word,
Excel, PowerPoint, PDF, Markdown or text. A search returns the
best-matching sections, each named by its document and headings; a read
returns one section or the whole document with line numbers. What is
returned passes the same redaction as every other result. Nothing is
written to the folders.

`get_runbook` lists the runbooks, a runbook's Parts and where each
stands, or one Part's steps in order. For each step it gives its kind
(check, config, exec, manual), the devices it names matched to the open
project, its commands exactly as written, what to expect, its recorded
status, the Part's rollback and the next step not done. `record_step`
records how a step went (done, skipped, failed, or pending to undo) in
this project's record, and touches no device. It is the one tool besides
the proposals that writes, so it is not marked read-only. A change a
runbook step asks for is still a `propose_config_change` that waits for
the engineer.

