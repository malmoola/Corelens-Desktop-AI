// Diagnose network against a stubbed backend that records what the page
// sends and lets the harness play Rust's events: the top bar's Diagnose
// and its form, live progress per device and Stop, the report (summary,
// ranked causes with evidence, fix plan, findings, what could not be
// checked), Review fix plan opening the approval dialog, Propose again,
// Markdown and PDF export, Ask AI to explain, and the Diagnose buttons on a
// device, a link, the inspector and two selected devices. Invented names
// and documentation-range addresses only.
import { chromium } from "playwright";

const URL = process.env.CV_URL ?? "http://localhost:5173/";
const NOW = 1756000000000;

let failures = 0;
const check = (name, ok, detail = "") => {
  if (ok) console.log(`ok   ${name}`);
  else { failures++; console.log(`FAIL ${name}${detail ? `  ${detail}` : ""}`); }
};

const project = {
  meta: { id: "p1", name: "Example project", createdAt: NOW, updatedAt: NOW },
  document: {
    nodes: [
      { id: "n1", type: "device", position: { x: 260, y: 220 }, data: { label: "SW1", deviceType: "switch", site: "HQ", tags: [], addresses: [{ id: "a1", label: "Management", address: "192.0.2.10", isPrimary: true }] } },
      { id: "n2", type: "device", position: { x: 620, y: 220 }, data: { label: "SW2", deviceType: "switch", site: "HQ", tags: [], addresses: [{ id: "a2", label: "Management", address: "192.0.2.11", isPrimary: true }] } },
    ],
    edges: [{
      id: "e1", source: "n1", target: "n2", sourceHandle: "r", targetHandle: "l", type: "live",
      data: { sourcePortLabel: "Gi0/1", targetPortLabel: "Gi1/0/1", label: "", pathType: "straight", direction: "none", width: 2, color: "#7c8fa3", enabled: true, maintenance: false, lineStyle: "solid", healthRule: { type: "manual", manualStatus: "healthy" } },
    }],
    probes: [],
    canvas: {},
  },
};

const report = {
  id: "d1",
  scope: { kind: "project" },
  depth: "quick",
  symptom: "phones get no IP",
  startedMs: NOW,
  finishedMs: NOW + 9000,
  cancelled: false,
  summary: ["1 of 2 device(s) in the whole project checked (quick run).", "2 critical and 0 warning finding(s); 31 check(s) passed.", "Most likely cause: VLAN 20 is not allowed on SW2 Gi1/0/1 (the trunk to SW1) (95% confidence)."],
  devices: [
    { name: "SW1", address: "192.0.2.10", platform: "cisco_ios", reached: true, error: null, commandsRun: 22, commandsFailed: 0 },
    { name: "SW2", address: "192.0.2.11", platform: "", reached: false, error: "There is no route from this computer to 192.0.2.11", commandsRun: 0, commandsFailed: 0 },
  ],
  causes: [{ id: "trunk_vlan_pruned|x.trunk_vlan_mismatch|sw2|gi1/0/1|20", title: "VLAN 20 is not allowed on SW2 Gi1/0/1 (the trunk to SW1)", severity: "critical", confidence: 0.95, why: "The VLAN exists on both switches but only one end of the trunk carries it.", findings: ["x.trunk_vlan_mismatch|sw2|gi1/0/1|20", "x.fhrp_split|sw1|192.0.2.129"], supporting: ["SW2: allowed: 1,10", "SW1 — SW1, SW2 are all active for gateway 192.0.2.129"], rulingOut: ["No interface errors on SW2, so it is not a bad cable."] }],
  findings: [
    { id: "x.trunk_vlan_mismatch|sw2|gi1/0/1|20", check: "x.trunk_vlan_mismatch", title: "VLAN missing on one end of a trunk", layer: "l2", severity: "critical", device: "SW2", says: "SW2 Gi1/0/1 does not carry VLAN 20", fields: {}, evidence: ["allowed: 1,10"] },
    { id: "x.fhrp_split|sw1|192.0.2.129", check: "x.fhrp_split", title: "Gateway redundancy split", layer: "l3", severity: "critical", device: "SW1", says: "SW1, SW2 are all active for gateway 192.0.2.129", fields: {}, evidence: [] },
  ],
  passed: [{ check: "l2.err_disabled", title: "Port err-disabled", layer: "l2", devices: ["SW1"], says: "No port is err-disabled." }],
  notChecked: [{ device: "SW2", check: "*", why: "There is no route from this computer to 192.0.2.11" }],
  plan: [
    { seq: 1, cause: "trunk_vlan_pruned|x", device: "SW2", kind: "change", title: "Allow VLAN 20 on Gi1/0/1 of SW2", commands: ["interface Gi1/0/1", " switchport trunk allowed vlan add 20"], verify: ["show interfaces Gi1/0/1 switchport"], impact: "VLAN 20 is carried again.", rollback: "Snapshot first; one click rolls back.", risk: "Low: one interface", status: "proposed", note: "", proposalId: "c1" },
    { seq: 2, cause: "err|x", device: "SW1", kind: "change", title: "Fix by hand on SW1", commands: [], verify: [], impact: "", rollback: "", risk: "", status: "manual", note: "What the device said for interface is not a value Corelens will put into a command.", proposalId: null },
  ],
  diff: { previousId: "d0", previousMs: NOW - 86400000, new: ["x.trunk_vlan_mismatch|sw2|gi1/0/1|20"], resolved: ["SW1 — Gi0/3 is err-disabled (bpduguard)"], unchanged: 0 },
  path: null,
  playbooks: ["cisco_ios@1"],
  tables: {},
};

const change = (id, status) => ({
  id, projectId: "p1", kind: "change", source: "diagnose", proposer: "Corelens Diagnose (fix plan of run d1)", reason: "Diagnosis d1 — fix plan step 1",
  createdMs: Date.now(), deadlineMs: Date.now() + 600000, status, decidedMs: null, decidedBy: "", note: "",
  targets: [{ seq: 0, device: "SW2", address: "192.0.2.11", platform: "ciscoIos", platformName: "Cisco IOS / IOS-XE", commands: ["interface Gi1/0/1", " switchport trunk allowed vlan add 20"], verifyCommands: [], assessment: { class: "low", hardBlocks: [], summary: "" }, predicted: [], predictedIsModel: true, rollbackPlan: "", status: status === "pending" ? "pending" : "denied", message: "", snapshot: "", actual: [], push: [], verification: [], rollback: [], confirmByMs: null, updatedMs: NOW }],
});

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
await page.addInitScript(({ p, rep }) => {
  window.__calls = [];
  window.__listeners = {};
  let next = 1;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    transformCallback(cb) { const id = next++; window[`_${id}`] = cb; return id; },
    invoke(cmd, args = {}) {
      window.__calls.push({ cmd, args });
      if (cmd === "plugin:event|listen") { const eid = next++; (window.__listeners[args.event] ??= []).push({ eid, handler: args.handler }); return Promise.resolve(eid); }
      if (cmd === "plugin:event|unlisten") { window.__listeners[args.event] = (window.__listeners[args.event] ?? []).filter((l) => l.eid !== args.eventId); return Promise.resolve(); }
      const meta = { id: p.meta.id, name: p.meta.name, customer: "", site: "", ticket: "", engineer: "", description: "", created_at: p.meta.createdAt, updated_at: p.meta.updatedAt, archived: false };
      if (cmd === "list_projects") return Promise.resolve([meta]);
      if (cmd === "save_project") return Promise.resolve();
      if (cmd === "load_project") return Promise.resolve({ meta, document_version: 1, document: p.document });
      if (cmd === "get_settings") return Promise.resolve({});
      if (cmd === "app_info") return Promise.resolve({ version: "2.13.0", dataDir: "/nowhere", documentVersion: 1 });
      if (cmd === "mcp_status") return Promise.resolve({ enabled: false, listening: false, error: null, executable: "x", claudeCodeCommand: "x", claudeDesktopConfig: "{}" });
      if (cmd === "ai_settings") return Promise.resolve({ chatEnabled: true, provider: "anthropic", models: { anthropic: "claude-opus-5-5", openai: "x", ollama: "x" }, keys: { anthropic: true, openai: false } });
      if (cmd === "ai_chat_history") return Promise.resolve({ rows: [], session: { inputTokens: 0, outputTokens: 0 }, running: false });
      if (cmd === "diagnose_start") return Promise.resolve(`d${window.__calls.filter((c) => c.cmd === "diagnose_start").length}`);
      if (cmd === "diagnose_cancel") return Promise.resolve(true);
      if (cmd === "diagnose_list") return Promise.resolve([{ id: "d0", scope: "the whole project", startedMs: Date.now() - 86400000, source: "operator", critical: 1, warning: 2, topCause: "Gi0/3 is err-disabled", cancelled: false }]);
      if (cmd === "diagnose_get") return Promise.resolve(rep);
      if (cmd === "diagnose_export_md") return Promise.resolve("# Diagnosis — Example project\n");
      if (cmd === "diagnose_export_pdf") return Promise.resolve([37, 80, 68, 70]);
      if (cmd === "diagnose_propose" || cmd === "change_decide") return Promise.resolve(null);
      if (cmd === "pick_export_target") return Promise.resolve({ token: "t1", path: "/nowhere/x" });
      if (cmd === "save_export") return Promise.resolve();
      return Promise.resolve([]);
    },
  };
  window.__emit = (event, payload) => {
    for (const { handler } of window.__listeners[event] ?? []) if (window[`_${handler}`]) window[`_${handler}`]({ event, id: handler, payload });
  };
}, { p: project, rep: report });
page.on("pageerror", (e) => console.log("PAGE EXCEPTION:", String(e).slice(0, 300)));
await page.goto(URL, { waitUntil: "networkidle" });
await page.waitForTimeout(500);
await page.locator(".cv-project-open").first().click();
await page.waitForTimeout(800);

const sent = (cmd) => page.evaluate((c) => window.__calls.filter((x) => x.cmd === c), cmd);
const emit = (event, payload) => page.evaluate(([e, p]) => window.__emit(e, p), [event, payload]);
const progress = (over) => ({ runId: "d1", projectId: "p1", kind: "device", scope: "the whole project", devices: [], device: "", state: "", done: 0, total: 0, command: "", summary: [], error: "", ...over });
const panel = page.locator('[data-region="diagnose"]');

// ------------------------------------------------- the form
check("the top bar has Diagnose", (await page.locator('[data-action="diagnose-open"]').count()) === 1);
check("the panel is closed until asked for", (await panel.count()) === 0);
await page.locator('[data-action="diagnose-open"]').click();
await page.waitForTimeout(300);
check("Diagnose opens the panel on the whole project", (await panel.count()) === 1 && (await panel.locator('[data-field="diagnose-scope-project"]').isChecked()));
check("it lists earlier diagnoses", /Gi0\/3 is err-disabled/.test(await panel.locator('[data-region="diagnose-history"]').innerText()));
check("it says it only reads", /Read-only/.test(await panel.innerText()));
await panel.locator('[data-field="diagnose-scope-site"]').check();
check("one site offers the project's sites", (await panel.locator('[data-field="diagnose-site"] option', { hasText: "HQ" }).count()) === 1);
check("and waits for one to be chosen", await panel.locator('[data-action="diagnose-run"]').isDisabled());
await panel.locator('[data-field="diagnose-scope-project"]').check();
await panel.locator('[data-field="diagnose-depth"]').selectOption("full");
await panel.locator('[data-field="diagnose-symptom"]').fill("phones get no IP");
await panel.locator('[data-action="diagnose-run"]').click();
await page.waitForTimeout(300);
const started = (await sent("diagnose_start")).at(-1)?.args;
check("Run sends the scope, the depth and the symptom", started?.scope === "project" && started.depth === "full" && started.symptom === "phones get no IP", JSON.stringify(started));

// ------------------------------------------------- live progress and Stop
await emit("corelens://diagnose", progress({ kind: "started", devices: ["SW1", "SW2"] }));
await emit("corelens://diagnose", progress({ device: "SW1", state: "running", done: 4, total: 22, command: "show vlan brief" }));
await emit("corelens://diagnose", progress({ device: "SW2", state: "unreachable", error: "There is no route from this computer to 192.0.2.11" }));
await page.waitForTimeout(250);
const live = panel.locator('[data-region="diagnose-progress"]');
check("each device's progress is shown as it arrives", /SW1 reading · 4\/22 · show vlan brief/.test(await live.innerText()), await live.innerText());
check("a device not reached says why", (await live.locator('[data-device="SW2"][data-state="unreachable"]').count()) === 1 && /no route/.test(await live.innerText()));
const bar = Number(await live.locator("progress").getAttribute("value"));
check("the bar counts finished devices and commands", bar > 0.5 && bar < 0.6, String(bar));
await panel.locator('[data-action="diagnose-cancel"]').click();
await page.waitForTimeout(200);
check("Stop asks Rust to stop that run", (await sent("diagnose_cancel")).at(-1)?.args.runId === "d1");

// ------------------------------------------------- the report
await emit("corelens://diagnose", progress({ kind: "finished", summary: report.summary }));
await page.waitForTimeout(400);
const rep = panel.locator('[data-region="diagnose-report"]');
check("when it finishes the report is fetched and shown", (await sent("diagnose_get")).at(-1)?.args.runId === "d1" && (await rep.count()) === 1);
check("the summary leads", /Most likely cause: VLAN 20/.test(await rep.locator('[data-region="diagnose-summary"]').innerText()));
check("causes are ranked with their confidence", /95% confidence/.test(await rep.locator('[data-cause="trunk_vlan_pruned"]').innerText()));
await rep.locator('[data-cause="trunk_vlan_pruned"] details summary').first().click();
check("with the evidence behind them", /allowed: 1,10/.test(await rep.locator('[data-cause="trunk_vlan_pruned"]').innerText()));
check("the plan shows the exact commands", /switchport trunk allowed vlan add 20/.test(await rep.locator('[data-region="diagnose-plan"] [data-step="1"]').innerText()));
check("a step without a safe value is by hand, with no commands", (await rep.locator('[data-step="2"][data-state="manual"] pre').count()) === 0 && /by hand/.test(await rep.locator('[data-step="2"]').innerText()));
check("Review fix plan waits for a proposal to be pending", await rep.locator('[data-action="diagnose-review"]').isDisabled());
await emit("corelens://change", change("c1", "pending"));
await page.waitForTimeout(300);
check("a fix plan's proposal does not cover the report by itself", (await page.locator('[data-region="change-approval"]').count()) === 0);
check("a pending step says it waits for approval", (await rep.locator('[data-step="1"][data-state="pending"]').count()) === 1);
check("Review fix plan counts what waits", /Review fix plan \(1\)/.test(await rep.locator('[data-action="diagnose-review"]').innerText()));
await rep.locator('[data-action="diagnose-review"]').click();
await page.waitForTimeout(300);
check("and opens the approval dialog on it", (await page.locator('[data-region="change-approval"]').count()) === 1 && /SW2/.test(await page.locator('[data-region="change-approval"]').innerText()));
check("nothing was decided by opening it", (await sent("change_decide")).length === 0);
await page.locator('[data-region="change-approval"] [data-action="change-deny"]').click().catch(() => undefined);
await page.waitForTimeout(200);
await emit("corelens://change", change("c1", "denied"));
await page.locator('[data-region="change-approval"] [data-action="change-close"]').click().catch(() => undefined);
await page.waitForTimeout(300);
check("a denied step can be proposed again", (await rep.locator('[data-step="1"][data-state="denied"] [data-action="diagnose-propose-again"]').count()) === 1);
await rep.locator('[data-action="diagnose-propose-again"]').click();
await page.waitForTimeout(200);
check("Propose again names the run and the step", JSON.stringify((await sent("diagnose_propose")).at(-1)?.args) === '{"runId":"d1","seq":1}');
check("findings are grouped worst first", (await rep.locator('details[data-severity="critical"] [data-check="x.trunk_vlan_mismatch"]').count()) === 1);
check("what could not be checked is open when a device was not reached", /SW2.*no route/s.test(await rep.locator('[data-region="diagnose-not-checked"]').innerText()));
check("the change since the last run is shown", /1 new, 1 resolved, 0 unchanged/.test(await rep.innerText()));
await rep.locator('[data-action="diagnose-export-md"]').click();
await page.waitForTimeout(250);
await rep.locator('[data-action="diagnose-export-pdf"]').click();
await page.waitForTimeout(250);
check("Markdown and PDF are made by Rust and saved", (await sent("diagnose_export_md")).length === 1 && (await sent("diagnose_export_pdf")).length === 1 && (await sent("save_export")).length >= 2);
await rep.locator('[data-action="diagnose-explain"]').click();
await page.waitForTimeout(300);
const ask = await page.locator('[data-field="ai-chat-input"]').inputValue().catch(() => "");
check("Ask AI to explain opens the chat with the question written", /diagnosis d1/.test(ask) && /VLAN 20/.test(ask), ask);
check("and sends nothing until the engineer does", (await sent("ai_chat_send")).length === 0);
await page.locator('[data-region="ai-chat-collapse"]').click().catch(() => undefined);

// ------------------------------------------------- the other Diagnose buttons
await panel.locator('[data-region="diagnose-close"]').click();
await page.waitForTimeout(200);
check("the panel closes", (await panel.count()) === 0);
const before = (await sent("diagnose_start")).length;
await page.locator(".react-flow__node", { hasText: "SW1" }).first().click({ button: "right" });
await page.waitForTimeout(200);
await page.locator(".cv-menu button", { hasText: "Diagnose this device" }).click();
await page.waitForTimeout(300);
check("a device's menu diagnoses it", JSON.stringify((await sent("diagnose_start")).at(-1)?.args.targets) === '["SW1"]' && (await sent("diagnose_start")).length === before + 1);
check("and the panel opens on its progress", (await panel.locator('[data-region="diagnose-progress"]').count()) === 1);
await panel.locator('[data-region="diagnose-close"]').click();
await page.waitForTimeout(200);
await page.locator(`.cv-edge-hit[data-id="e1"] path`).first().click({ button: "right", force: true });
await page.waitForTimeout(200);
const linkItem = page.locator(".cv-menu button", { hasText: "Diagnose this link" });
if (await linkItem.count()) {
  await linkItem.click();
  await page.waitForTimeout(300);
}
check("a link's menu diagnoses both its ends", JSON.stringify((await sent("diagnose_start")).at(-1)?.args.targets) === '["SW1","SW2"]', JSON.stringify((await sent("diagnose_start")).at(-1)?.args));
await panel.locator('[data-region="diagnose-close"]').click().catch(() => undefined);
await page.waitForTimeout(200);
await page.keyboard.press("Escape");
await page.locator(".react-flow__node", { hasText: "SW1" }).first().click();
await page.waitForTimeout(200);
await page.locator('[data-action="diagnose-device"]').click();
await page.waitForTimeout(300);
check("the inspector's Diagnose runs on that device", JSON.stringify((await sent("diagnose_start")).at(-1)?.args.targets) === '["SW1"]' && (await sent("diagnose_start")).length === before + 3);
await panel.locator('[data-region="diagnose-close"]').click().catch(() => undefined);
await page.waitForTimeout(200);
await page.locator(".react-flow__node", { hasText: "SW1" }).first().click();
await page.locator(".react-flow__node", { hasText: "SW2" }).first().click({ modifiers: ["Control"] });
await page.waitForTimeout(200);
await page.locator(".react-flow__node", { hasText: "SW2" }).first().click({ button: "right" });
await page.waitForTimeout(200);
await page.locator(".cv-menu button", { hasText: "Diagnose the path between these two" }).click();
await page.waitForTimeout(300);
check("two selected devices offer the path between them, in the form", (await panel.locator('[data-field="diagnose-scope-path"]').isChecked()) && (await panel.locator('[data-field="diagnose-src"]').inputValue()) !== "" && (await panel.locator('[data-field="diagnose-dst"]').inputValue()) !== "");
await panel.locator('[data-field="diagnose-port"]').fill("8443");
await panel.locator('[data-action="diagnose-run"]').click();
await page.waitForTimeout(300);
const path = (await sent("diagnose_start")).at(-1)?.args;
check("and runs it with the port", path?.scope === "path" && [path.src, path.dst].sort().join() === "SW1,SW2" && path.port === 8443, JSON.stringify(path));

await browser.close();
console.log(failures === 0 ? "\nall ok" : `\n${failures} failure(s)`);
process.exit(failures === 0 ? 0 : 1);
