// Settings → AI: letting Claude use Corelens through MCP. Off until
// switched on; the setup commands shown with this installation's path; the
// list of what Claude asked and was given, its export and clearing; and the
// status strip naming the tool while a call runs. The stub stands in for
// the backend and records every message the page sends. Invented names and
// documentation-range addresses only.
import { chromium } from "playwright";

const URL = process.env.CV_URL ?? "http://localhost:5173/";
const NOW = 1756000000000;

let failures = 0;
const check = (name, ok, detail = "") => {
  if (ok) console.log(`ok   ${name}`);
  else { failures++; console.log(`FAIL ${name}${detail ? `  ${detail}` : ""}`); }
};

const project = {
  meta: { id: "ai", name: "Example project", customer: "", site: "", ticket: "", engineer: "", description: "", createdAt: NOW, updatedAt: NOW, archived: false },
  documentVersion: 1,
  document: { nodes: [], edges: [], probes: [], canvas: {} },
};

const EXE = "C:\\Program Files\\Corelens Desktop AI\\corelens-desktop-ai.exe";
const ACTIVITY = [
  { id: 2, atMs: NOW + 2000, projectId: "ai", tool: "run_show_command", arguments: '{"device":"SW1","command":"show ip arp"}', device: "SW1", address: "192.0.2.10", command: "show ip arp", outcome: "ok", result: "SW1 (192.0.2.10) · Cisco IOS / IOS-XE · `show ip arp`\nInternet  192.0.2.1  0  0000.0000.0001  ARPA  Vlan10\n", bytes: 90, durationMs: 1200 },
  { id: 1, atMs: NOW + 1000, projectId: "ai", tool: "get_config", arguments: '{"device":"SW1"}', device: "SW1", address: "", command: "", outcome: "withheld", result: "This result was withheld", bytes: 24, durationMs: 4 },
];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
await page.addInitScript(({ p, exe, activity }) => {
  window.__calls = [];
  window.__enabled = false;
  window.__activity = activity;
  window.__listeners = {};
  let next = 1;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  const status = () => ({
    enabled: window.__enabled,
    listening: window.__enabled,
    error: null,
    executable: exe,
    claudeCodeCommand: `claude mcp add --transport stdio --scope user corelens -- "${exe}" --mcp`,
    claudeDesktopConfig: JSON.stringify({ mcpServers: { corelens: { command: exe, args: ["--mcp"] } } }, null, 2),
  });
  window.__TAURI_INTERNALS__ = {
    transformCallback(cb) { const id = next++; window[`_${id}`] = cb; return id; },
    invoke(cmd, args = {}) {
      window.__calls.push({ cmd, args });
      if (cmd === "plugin:event|listen") { (window.__listeners[args.event] ??= []).push(args.handler); return Promise.resolve(next++); }
      const meta = { id: p.meta.id, name: p.meta.name, customer: "", site: "", ticket: "", engineer: "", description: "", created_at: p.meta.createdAt, updated_at: p.meta.updatedAt, archived: false };
      if (cmd === "list_projects") return Promise.resolve([meta]);
      if (cmd === "save_project") return Promise.resolve();
      if (cmd === "load_project") return Promise.resolve({ meta, document_version: p.documentVersion, document: p.document });
      if (cmd === "get_settings") return Promise.resolve({});
      if (cmd === "app_info") return Promise.resolve({ version: "2.13.0", dataDir: "/nowhere", documentVersion: 1 });
      if (cmd === "mcp_status") return Promise.resolve(status());
      if (cmd === "mcp_set_enabled") { window.__enabled = !!args.enabled; return Promise.resolve(status()); }
      if (cmd === "mcp_activity") return Promise.resolve(window.__activity);
      if (cmd === "mcp_clear_activity") { const n = window.__activity.length; window.__activity = []; return Promise.resolve(n); }
      if (cmd === "mcp_change_record") return Promise.resolve("# Change record — Example project\n");
      if (cmd === "pick_export_target") return Promise.resolve({ token: "t1", path: "/nowhere/example-project-ai-change-record.md" });
      if (cmd === "save_export") return Promise.resolve();
      return Promise.resolve([]);
    },
  };
  window.__emit = (event, payload) => {
    for (const id of window.__listeners[event] ?? []) {
      if (window[`_${id}`]) window[`_${id}`]({ event, id, payload });
    }
  };
}, { p: project, exe: EXE, activity: ACTIVITY });
page.on("pageerror", (e) => console.log("PAGE EXCEPTION:", String(e).slice(0, 300)));
await page.goto(URL, { waitUntil: "networkidle" });
await page.waitForTimeout(500);
await page.locator(".cv-project-open").first().click();
await page.waitForTimeout(700);
await page.locator(".cv-nav-item", { hasText: /^Settings$/ }).click();
await page.waitForTimeout(500);

const ai = page.locator('[data-region="ai"]');
const sent = (cmd) => page.evaluate((c) => window.__calls.filter((x) => x.cmd === c), cmd);

// ------------------------------------------------- off by default
check("Settings has an AI block, listed in the nav", (await ai.count()) === 1 && (await page.locator(".cv-settings-nav", { hasText: "AI" }).count()) === 1);
check("it says Claude never connects to a device", /Claude never connects to a device/.test(await ai.innerText()), (await ai.innerText()).slice(0, 200));
check("and what reaches Anthropic, and that Corelens holds no Anthropic key", /reaches Anthropic through the Claude app/.test(await ai.innerText()) && /holds no Anthropic key/.test(await ai.innerText()));
check("the switch is off until somebody turns it on", !(await ai.locator('[data-field="mcp-enabled"]').isChecked()));
check("and says nothing is listening", (await ai.locator('[data-hint="mcp-state"]').getAttribute("data-tone")) === "off");
check("nothing was asked to listen on load", (await sent("mcp_set_enabled")).length === 0);
check("the setup commands are not shown while off", (await ai.locator('[data-region="ai-setup"]').count()) === 0);

// ------------------------------------------------- switching it on
await ai.locator('[data-field="mcp-enabled"]').check();
await page.waitForTimeout(300);
const on = await sent("mcp_set_enabled");
check("switching on sends one request to listen", on.length === 1 && on[0].args.enabled === true, JSON.stringify(on));
check("and the state says an MCP client can use the open project", (await ai.locator('[data-hint="mcp-state"]').getAttribute("data-tone")) === "on");
const code = await ai.locator('[data-field="claude-code-command"]').innerText();
check("the Claude Code command names this installation, with --mcp", code === `claude mcp add --transport stdio --scope user corelens -- "${EXE}" --mcp`, code);
const desktop = JSON.parse(await ai.locator('[data-field="claude-desktop-config"]').innerText());
check("and the Claude Desktop entry starts the same executable", desktop.mcpServers.corelens.command === EXE && desktop.mcpServers.corelens.args[0] === "--mcp");

// ------------------------------------------------- what Claude asked
const rows = ai.locator('[data-region="ai-activity"] li');
check("every call for the project is listed, newest first", (await rows.count()) === 2 && (await rows.first().getAttribute("data-tool")) === "run_show_command");
check("a call names its device and command", /run_show_command.*on SW1.*show ip arp.*answered/s.test(await rows.first().innerText()), await rows.first().innerText());
check("a withheld result says why", /withheld \(looked like a secret\)/.test(await rows.nth(1).innerText()));
await rows.first().locator("summary").click();
check("and opens to exactly what Claude was given", /Internet  192\.0\.2\.1/.test(await rows.first().locator(".cv-ai-result").innerText()));

await ai.locator('[data-action="ai-export"]').click();
await page.waitForTimeout(300);
check("the change record is saved through the save dialog", (await sent("mcp_change_record")).length === 1 && (await sent("save_export")).length === 1);
check("under the project's name", (await sent("pick_export_target"))[0]?.args.filename === "example-project-ai-change-record.md", JSON.stringify(await sent("pick_export_target")));

// ------------------------------------------------- the status strip
check("nothing in the top bar while Claude is idle", (await page.locator('[data-region="ai-busy"]').count()) === 0);
await page.evaluate(() => window.__emit("corelens://mcp-activity", { tool: "run_show_command", device: "SW1", running: true, outcome: "" }));
await page.waitForTimeout(150);
check("a running call is named in the top bar, with its device", (await page.locator('[data-region="ai-busy"]').innerText()) === "Claude: run_show_command on SW1\u2026", await page.locator('[data-region="ai-busy"]').innerText().catch(() => ""));
await page.evaluate(() => window.__emit("corelens://mcp-activity", { tool: "run_show_command", device: "SW1", running: false, outcome: "ok" }));
await page.waitForTimeout(150);
check("and goes when it ends", (await page.locator('[data-region="ai-busy"]').count()) === 0);

// ------------------------------------------------- clearing, switching off
await ai.locator('[data-action="ai-clear"]').click();
await page.waitForTimeout(300);
check("clearing empties the list", (await ai.locator('[data-hint="ai-no-activity"]').count()) === 1 && (await sent("mcp_clear_activity")).length === 1);
await ai.locator('[data-field="mcp-enabled"]').uncheck();
await page.waitForTimeout(300);
const off = await sent("mcp_set_enabled");
check("switching off asks the bridge to stop", off.length === 2 && off[1].args.enabled === false);
check("and the setup commands go with it", (await ai.locator('[data-region="ai-setup"]').count()) === 0);

await browser.close();
console.log(failures === 0 ? "\nall ok" : `\n${failures} failure(s)`);
process.exit(failures === 0 ? 0 : 1);
