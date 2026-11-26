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
  { id: 2, atMs: NOW + 2000, projectId: "ai", tool: "run_show_command", arguments: '{"target":"SW1","command":"show ip arp"}', device: "SW1", address: "192.0.2.10", command: "show ip arp", outcome: "ok", result: "SW1 (192.0.2.10) · Cisco IOS / IOS-XE · `show ip arp`\nInternet  192.0.2.1  0  0000.0000.0001  ARPA  Vlan10\n", bytes: 90, durationMs: 1200 },
  { id: 1, atMs: NOW + 1000, projectId: "ai", tool: "get_config", arguments: '{"target":"SW1"}', device: "SW1", address: "", command: "", outcome: "withheld", result: "This result was withheld", bytes: 24, durationMs: 4 },
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
    // As Rust writes them for an executable whose path has a space.
    setups: [
      { client: "claudeCode", text: `claude mcp add --transport stdio --scope user corelens -- "${exe}" --mcp` },
      { client: "claudeDesktop", text: JSON.stringify({ mcpServers: { corelens: { command: exe, args: ["--mcp"] } } }, null, 2) },
      { client: "vscode", text: JSON.stringify({ servers: { corelens: { type: "stdio", command: exe, args: ["--mcp"] } } }, null, 2) },
      { client: "copilotCli", text: `copilot mcp add corelens -- "${exe}" --mcp` },
      { client: "visualStudio", text: JSON.stringify({ servers: { corelens: { type: "stdio", command: exe, args: ["--mcp"] } } }, null, 2) },
      { client: "codex", text: `[mcp_servers.corelens]\ncommand = ${JSON.stringify(exe)}\nargs = ["--mcp"]` },
      { client: "cursor", text: JSON.stringify({ mcpServers: { corelens: { command: exe, args: ["--mcp"] } } }, null, 2) },
      { client: "windsurf", text: JSON.stringify({ mcpServers: { corelens: { command: exe, args: ["--mcp"] } } }, null, 2) },
      { client: "gemini", text: JSON.stringify({ mcpServers: { corelens: { command: exe, args: ["--mcp"] } } }, null, 2) },
      { client: "other", text: `"${exe}" --mcp` },
    ],
  });
  window.__copied = [];
  Object.defineProperty(navigator, "clipboard", { value: { writeText: (x) => { window.__copied.push(x); return Promise.resolve(); } }, configurable: true });
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
      // The rest of Settings → AI, as Rust answers it with nothing set up.
      if (cmd === "ai_settings") return Promise.resolve({ chatEnabled: false, provider: "anthropic", models: { anthropic: "claude-opus-5-5", openai: "gpt-6-astra", ollama: "qwen3" }, openaiBaseUrl: "https://api.openai.com/v1", openaiStyle: "responses", ollamaBaseUrl: "http://localhost:11434", maxToolCalls: 15, toolOutputChars: 12000, chatRetentionDays: 90, auditRetentionDays: 90, proposalTimeoutMinutes: 10, keys: { anthropic: false, openai: false } });
      if (cmd === "app_log_info") return Promise.resolve({ folder: "/nowhere/logs", files: [], totalBytes: 0, limitBytes: 52428800, verboseUntilMs: 0 });
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
// The application's settings are on the start screen, before any project.
await page.locator('[data-action="app-settings"]').click();
await page.waitForTimeout(500);

const ai = page.locator('[data-region="ai"]');
const sent = (cmd) => page.evaluate((c) => window.__calls.filter((x) => x.cmd === c), cmd);

// ------------------------------------------------- off by default
check("the start screen's Settings has an AI block, listed in the nav", (await ai.count()) === 1 && (await page.locator(".cv-settings-nav", { hasText: "AI" }).count()) === 1);
check("it names more than one assistant, and that none connects to a device",
  /Claude, GitHub Copilot, ChatGPT and Codex, Cursor, Gemini/.test(await ai.innerText()) && /The assistant never connects to a device/.test(await ai.innerText()), (await ai.innerText()).slice(0, 300));
check("and what reaches the company behind it, and that this part of Corelens sends nothing there itself",
  /reaches the company behind it \(Anthropic, GitHub, OpenAI, Google…\)/.test(await ai.innerText()) && /sends nothing to any of them itself and holds no key/.test(await ai.innerText()));
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
const setupShown = () => ai.locator('[data-field="mcp-setup-text"]').innerText();
const clients = await ai.locator('[data-field="mcp-client"] option').allInnerTexts();
check("one dropdown names every assistant it can set up",
  JSON.stringify(clients) === JSON.stringify(["Claude Code", "Claude Desktop", "GitHub Copilot in VS Code", "GitHub Copilot CLI", "GitHub Copilot in Visual Studio or JetBrains", "ChatGPT desktop / OpenAI Codex", "Cursor", "Windsurf", "Gemini CLI", "Any other MCP client"]), JSON.stringify(clients));
check("and one setup is shown at a time", (await ai.locator('[data-field="mcp-setup-text"]').count()) === 1 && (await ai.locator('[data-action="copy-mcp-setup"]').count()) === 1);
check("Claude Code's command first, naming this installation, with --mcp", (await setupShown()) === `claude mcp add --transport stdio --scope user corelens -- "${EXE}" --mcp`, await setupShown());
await ai.locator('[data-field="mcp-client"]').selectOption("claudeDesktop");
const desktop = JSON.parse(await setupShown());
check("Claude Desktop's entry starts the same executable", desktop.mcpServers.corelens.command === EXE && desktop.mcpServers.corelens.args[0] === "--mcp");
await ai.locator('[data-field="mcp-client"]').selectOption("vscode");
const vscode = JSON.parse(await setupShown());
check("Copilot in VS Code: a stdio server under servers, and how to reach it in Agent mode",
  vscode.servers.corelens.type === "stdio" && vscode.servers.corelens.command === EXE && /Agent mode/.test(await ai.locator('[data-hint="mcp-client-hint"]').innerText()));
await ai.locator('[data-field="mcp-client"]').selectOption("codex");
check("ChatGPT desktop / Codex: the config.toml entry, and what a browser cannot do",
  (await setupShown()).startsWith("[mcp_servers.corelens]\ncommand = ") && /ChatGPT in a browser cannot start a program/.test(await ai.locator('[data-hint="mcp-client-hint"]').innerText()));
await ai.locator('[data-field="mcp-client"]').selectOption("copilotCli");
await ai.locator('[data-action="copy-mcp-setup"]').click();
await page.waitForTimeout(200);
check("Copy copies exactly the setup shown", (await page.evaluate(() => window.__copied.at(-1))) === `copilot mcp add corelens -- "${EXE}" --mcp`);
check("and says it did", (await ai.locator('[data-action="copy-mcp-setup"]').innerText()) === "Copied");
check("the choice is remembered", (await page.evaluate(() => localStorage.getItem("corelens.ai.mcpClient"))) === "copilotCli");

check("the start screen's AI block lists no project's activity", (await page.locator('[data-region="ai-mcp-activity"]').count()) === 0 && (await page.locator('[data-region="ai-changes"]').count()) === 0);

// ------------------------------------------------- what Claude asked, inside the project
await page.locator('[data-action="app-settings"]').click();
await page.waitForTimeout(300);
await page.locator(".cv-project-open").first().click();
await page.waitForTimeout(700);
await page.locator(".cv-nav-item", { hasText: /^Settings$/ }).click();
await page.waitForTimeout(500);
const mine = page.locator('[data-region="ai-project"]');
check("a project's Settings has AI in this project, and not the application's AI", (await mine.count()) === 1 && (await ai.count()) === 0 && (await page.locator('[data-field="mcp-enabled"]').count()) === 0);
const rows = mine.locator('[data-region="ai-activity"] li');
check("every call for the project is listed, newest first", (await rows.count()) === 2 && (await rows.first().getAttribute("data-tool")) === "run_show_command");
check("a call names its device and command", /run_show_command.*on SW1.*show ip arp.*answered/s.test(await rows.first().innerText()), await rows.first().innerText());
check("a withheld result says why", /withheld \(looked like a secret\)/.test(await rows.nth(1).innerText()));
await rows.first().locator("summary").click();
check("and opens to exactly what Claude was given", /Internet  192\.0\.2\.1/.test(await rows.first().locator(".cv-ai-result").innerText()));

await mine.locator('[data-action="ai-export"]').click();
await page.waitForTimeout(300);
check("the change record is saved through the save dialog", (await sent("mcp_change_record")).length === 1 && (await sent("save_export")).length === 1);
check("under the project's name", (await sent("pick_export_target"))[0]?.args.filename === "example-project-ai-change-record.md", JSON.stringify(await sent("pick_export_target")));

// ------------------------------------------------- the status strip
check("nothing in the top bar while Claude is idle", (await page.locator('[data-region="ai-busy"]').count()) === 0);
await page.evaluate(() => window.__emit("corelens://mcp-activity", { tool: "run_show_command", device: "SW1", running: true, outcome: "" }));
await page.waitForTimeout(150);
check("a running call is named in the top bar, with its device", (await page.locator('[data-region="ai-busy"]').innerText()) === "AI: run_show_command on SW1\u2026", await page.locator('[data-region="ai-busy"]').innerText().catch(() => ""));
await page.evaluate(() => window.__emit("corelens://mcp-activity", { tool: "run_show_command", device: "SW1", running: false, outcome: "ok" }));
await page.waitForTimeout(150);
check("and goes when it ends", (await page.locator('[data-region="ai-busy"]').count()) === 0);

// ------------------------------------------------- clearing, switching off
await mine.locator('[data-action="ai-clear"]').click();
await page.waitForTimeout(300);
check("clearing empties the list", (await mine.locator('[data-hint="ai-no-activity"]').count()) === 1 && (await sent("mcp_clear_activity")).length === 1);

// ------------------------------------------------- switching off, from the start screen again
await page.evaluate(() => window.__cvStore.getState().closeProject());
await page.waitForTimeout(700);
await page.locator('[data-action="app-settings"]').click();
await page.waitForTimeout(500);
await ai.locator('[data-field="mcp-enabled"]').uncheck();
await page.waitForTimeout(300);
const off = await sent("mcp_set_enabled");
check("switching off asks the bridge to stop", off.length === 2 && off[1].args.enabled === false);
check("and the setup commands go with it", (await ai.locator('[data-region="ai-setup"]').count()) === 0);

await browser.close();
console.log(failures === 0 ? "\nall ok" : `\n${failures} failure(s)`);
process.exit(failures === 0 ? 0 : 1);
