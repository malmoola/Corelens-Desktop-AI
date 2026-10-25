# Diagnose network

*Diagnose* runs a structured, read-only sweep of the devices you point it
at, tells you what is wrong and why it thinks so, and attaches a fix plan.
Every fix in the plan waits in Corelens as a proposal: **nothing changes on
any device until you approve that step** in the approval dialog, with all
the rules a change proposal has (a typed device name for anything that can
cut the device off, saving as its own approval, a snapshot and the
platform's rollback first).

## 1. Run it

From the window:

- **Diagnose** in the top bar — the whole project, one site, the selected
  devices, or the path between two endpoints.
- Right-click a device → *Diagnose this device*; a link → *Diagnose this
  link* (both its ends).
- Select two devices, right-click one → *Diagnose the path between these
  two*.
- The inspector's *Diagnose* button.

From the chat (*Ask AI*) or an MCP client (Claude Code, Claude Desktop):
ask "check this network, what's going on", "run a full diag on site HQ",
"why can't 192.0.2.40 reach the file server" — the model calls the
`run_diagnostic` tool, then explains from its report. `get_diagnosis` reads
a report run earlier, from any of these.

Choose **Quick** (health, interfaces, VLANs, spanning tree, routing
neighbours, gateways) or **Full** (also logs, optics, PoE, ARP, the
configuration against the saved one and the last backup). A symptom in your
own words ("floor 2 gets no IP", "the uplink is slow") weights the causes
toward what it is about; leave it empty rather than guess.

While it runs the panel shows each device — waiting, reading (which
command), read, or not reached and why — and **Stop** ends it, keeping what
was read.

## 2. What it checks

| Area | Checks |
| --- | --- |
| Management and health | reachability, uptime and reload reason, CPU, memory, fans, power supplies and temperature, flash space, log messages at severity 3 or worse in the last 24 hours |
| Layer 1 | ports that should be up and are down or shut (they carry a link in the diagram, or were up in the previous run), CRC/input/output errors, output drops, half duplex, speed/duplex mismatch across a link, flapping, weak optics, PoE faults and budget |
| Layer 2 | err-disabled ports and why, VLANs missing on one end of a trunk, native VLAN mismatch, the spanning-tree root (switches that disagree, a root on an access switch, a root that moved since the last run), recent topology changes, CDP/LLDP against the diagram |
| Layer 3 | SVIs down, default route or gateway, OSPF/BGP/EIGRP neighbours, HSRP/VRRP not converged, split (two active) or with none active, an address answering from two MACs, an address configured on two devices |
| Services | SVIs with clients but no DHCP relay, NTP synchronisation, RADIUS/TACACS+ server state |
| Firewalls | HA health and configuration sync, session table pressure, IPsec tunnels down, interface duplex; on PAN-OS the policy lookup for a flow |
| Path | the devices between the two endpoints over the diagram's links, and at each one: the ports up, the source's VLAN carried on each trunk, a route onward, an ACL on the way (named, not evaluated), the firewall's verdict; the first hop where it breaks |
| Configuration drift | running against startup (unsaved changes), running against the last backup Corelens took |

A device that cannot be reached, or drops partway, is reported — with the
reason — and the run goes on with the others. *What could not be checked*
lists every check that did not run and why (not reached, the device does
not have that command, a session that ended, a stopped run).

## 3. The report

- **Summary** — three to five lines: how many devices were read, how many
  findings, the most likely cause, and what changed since the last run of
  the same scope.
- **Likely root causes**, ranked, each with a confidence, why, the
  evidence behind it and what was ruled out. A cause explains the findings
  that follow from it: a VLAN missing on one end of a trunk also explains
  the gateway pair that went active-active on that VLAN.
- **Fix plan** — per cause, the exact commands per device, the risk as the
  change layer assesses it, the expected impact, how it is verified, and how
  it is rolled back. A step whose values (an interface name, a VLAN, an
  address) did not come from the device in a shape Corelens will put into a
  command is left for you to do by hand: device output is data, never
  trusted to be what it claims.
- **Findings** by severity, with the device's own lines as evidence (secrets
  removed), **what passed**, **what could not be checked**, and the
  **difference from the previous run**.
- **Path** — hop by hop, when the scope is a path.

*Export Markdown* and *Export PDF* save the report. *Ask AI to explain*
opens the chat with a question about it written in, for you to send.

## 4. The fix plan and your approval

When the run finishes, each step of the plan that has commands is proposed
— one proposal per step, from "Corelens Diagnose" — and waits. The approval
dialog does not open by itself over the report: **Review fix plan** opens
the first waiting step, and closing it after a decision goes on to the next.
For each one you see the commands, the diff against the running
configuration, the risk, and the rollback; approve or deny it. A step not
decided within the proposal time limit (10 minutes unless *Settings → AI*
says otherwise) counts as denied; *Propose again* in the report proposes it
afresh. Saving the configuration is always its own step.

## 5. Platforms

| Platform | Playbook |
| --- | --- |
| Cisco IOS / IOS-XE | the most complete |
| Cisco NX-OS | health, interfaces, err-disabled, trunks, CDP, VLANs, spanning-tree root, HSRP, OSPF, BGP, DHCP relay, drift |
| Aruba AOS-CX | health, fans, interfaces and trunks, LLDP, VLANs, OSPF, default route, drift |
| Juniper Junos | health, chassis alarms, interfaces and errors, LLDP, VLANs, OSPF, BGP, default route |
| FortiGate (FortiOS) | health, HA and its sync, sessions, interfaces, IPsec tunnels, default route, BGP |
| Palo Alto (PAN-OS) | health, HA and its sync, sessions, interfaces, IPsec tunnels, default route, policy lookup |

The playbooks are written from each vendor's command reference and proved
against recorded outputs in the shapes those references give; they have not
all met a device yet, and each says so (`verified: docs`). A report names
the playbooks and versions it was judged by.

Not done, and said so in the report where it matters: Junos lists trunk
VLANs per member, so the trunk comparison does not run on Junos; Junos,
FortiOS and PAN-OS save a change when it is committed, so there is no
unsaved check there; FortiOS's policy lookup needs the ingress interface and
is not asked; an ACL on the path is named, not evaluated; interface counters
count since they were last cleared.

## 6. What it reads and keeps

Every command is a read that the same allowlist as `run_show_command`
passes, and one of the playbook's own — no configuration mode, no clear, no
debug, nothing that writes the output anywhere. Corelens logs in with each
device's saved login over this computer's connection, six devices at a
time, each with its own time limit.

The report is kept in the project's database (every text in it already
through the redaction that removes passwords, keys and community strings),
deleted with the project, and pruned with the AI audit record's retention
(*Settings → AI → Retention*). The commands it ran are in the credential
use log, as for any login.

## 7. For contributors: the playbooks

`resources/diagnose/` holds one YAML file per platform, plus
`analyses.yaml` (the checks across devices) and `causes.yaml` (root causes,
what they explain, their fixes, and the symptom words). A playbook lists
commands (with the parser — an ntc template, line patterns or block
patterns — and the tables they feed), checks (the rule, severity, the
sentence, the evidence, the fix), fix templates (whose placeholders each
declare the shape a value must have) and how configurations are compared.
Each check must be silent on `fixtures/<platform>/healthy/` and fire on
`fixtures/<platform>/broken/`; `cargo test -p corelens-diagnose` fails
otherwise.
