// The in-app AI chat and the approval of a configuration change, against a
// stubbed backend that records every message the page sends and lets the
// harness play Rust's events: the Ask AI button and its shortcut, a turn
// streamed with a tool call and a parsed table, Stop, a question about a
// device from the inspector, the approval dialog with a hard block (the
// name typed), approve, deny, a failed verification rolled back, and
// Settings → AI in its four parts with nothing sent to a provider while
// the chat is off. Invented names and documentation-range addresses only.
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
    nodes: [{ id: "n1", type: "device", position: { x: 260, y: 220 }, data: { label: "SW1", deviceType: "switch", tags: [], addresses: [{ id: "a1", label: "Management", address: "192.0.2.10", isPrimary: true }] } }],
    edges: [],
    probes: [],
    canvas: {},
  },
};

const settings = (enabled) => ({
  chatEnabled: enabled,
  provider: "anthropic",
  models: { anthropic: "claude-opus-5-5", openai: "gpt-6-astra", ollama: "qwen3" },
  openaiBaseUrl: "https://api.openai.com/v1",
  openaiStyle: "responses",
  ollamaBaseUrl: "http://localhost:11434",
  maxToolCalls: 15,
  toolOutputChars: 12000,
  chatRetentionDays: 90,
  auditRetentionDays: 90,
  proposalTimeoutMinutes: 10,
  keys: { anthropic: false, openai: false },
});

const target = (over) => ({
  seq: 0, device: "SW1", address: "192.0.2.10", platform: "ciscoIos", platformName: "Cisco IOS / IOS-XE",
  commands: ["interface Vlan10", " shutdown"], verifyCommands: ["show interfaces status"],
  assessment: { class: "high", hardBlocks: [{ command: "shutdown", risk: "This changes Vlan10, which carries 192.0.2.10 — the address Corelens and the engineer reach the device on." }], summary: "1 line(s) can cut the device off or restart it; type the device's name to approve." },
  predicted: [{ mark: " ", text: "interface Vlan10" }, { mark: "+", text: " shutdown" }], predictedIsModel: true,
  rollbackPlan: "Snapshot copied to flash first; rollback with configure replace.", status: "pending", message: "", snapshot: "",
  actual: [], push: [], verification: [], rollback: [], confirmByMs: null, updatedMs: NOW,
  ...over,
});
const change = (id, over, targets) => ({
  id, projectId: "p1", kind: "change", source: "chat", proposer: "Corelens chat (anthropic, claude-opus-5-5)", reason: "test the dialog",
  createdMs: Date.now(), deadlineMs: Date.now() + 600000, status: "pending", decidedMs: null, decidedBy: "", note: "",
  targets: targets ?? [target({})], ...over,
});

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
await page.addInitScript(({ p, s0 }) => {
  window.__calls = [];
  window.__settings = s0;
  window.__listeners = {};
  let next = 1;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    transformCallback(cb) { const id = next++; window[`_${id}`] = cb; return id; },
    invoke(cmd, args = {}) {
      window.__calls.push({ cmd, args });
      // Listeners kept by the id listen returns, so unlisten removes them
      // the way Tauri does.
      if (cmd === "plugin:event|listen") { const eid = next++; (window.__listeners[args.event] ??= []).push({ eid, handler: args.handler }); return Promise.resolve(eid); }
      if (cmd === "plugin:event|unlisten") { window.__listeners[args.event] = (window.__listeners[args.event] ?? []).filter((l) => l.eid !== args.eventId); return Promise.resolve(); }
      const meta = { id: p.meta.id, name: p.meta.name, customer: "", site: "", ticket: "", engineer: "", description: "", created_at: p.meta.createdAt, updated_at: p.meta.updatedAt, archived: false };
      if (cmd === "list_projects") return Promise.resolve([meta]);
      if (cmd === "save_project") return Promise.resolve();
      if (cmd === "load_project") return Promise.resolve({ meta, document_version: 1, document: p.document });
      if (cmd === "get_settings") return Promise.resolve({});
      if (cmd === "app_info") return Promise.resolve({ version: "2.13.0", dataDir: "/nowhere", documentVersion: 1 });
      if (cmd === "mcp_status") return Promise.resolve({ enabled: false, listening: false, error: null, executable: "x", claudeCodeCommand: "x", claudeDesktopConfig: "{}" });
      if (cmd === "ai_settings") return Promise.resolve(window.__settings);
      if (cmd === "ai_setting_set") {
        if (args.key === "aiChatEnabled") window.__settings = { ...window.__settings, chatEnabled: args.value === "1" };
        if (args.key.startsWith("aiModel.")) window.__settings = { ...window.__settings, models: { ...window.__settings.models, [args.key.slice(8)]: args.value } };
        if (args.key === "aiSnowflakeAccount") window.__settings = { ...window.__settings, snowflakeAccount: args.value };
        if (args.key === "aiSnowflakeLibrary") window.__settings = { ...window.__settings, snowflakeLibrary: args.value };
        return Promise.resolve(window.__settings);
      }
      if (cmd === "ai_key_import") { window.__settings = { ...window.__settings, keys: { ...window.__settings.keys, [args.provider]: true } }; return Promise.resolve(window.__settings); }
      if (cmd === "ai_chat_history") return Promise.resolve({ rows: [], session: { inputTokens: 0, outputTokens: 0 }, running: false });
      if (cmd === "ai_chat_send" || cmd === "ai_chat_stop" || cmd === "change_decide") return Promise.resolve(cmd === "ai_chat_stop" ? true : null);
      if (cmd === "change_rollback") return Promise.resolve(window.__rolledBack);
      if (cmd === "change_export") return Promise.resolve("# Configuration changes\n");
      if (cmd === "ai_chat_clear") return Promise.resolve(0);
      if (cmd === "app_log_info") return Promise.resolve({ folder: "/nowhere/logs", files: [["corelens.log", 2048]], totalBytes: 2048, limitBytes: 52428800, verboseUntilMs: 0 });
      if (cmd === "app_log_verbose") return Promise.resolve({ folder: "/nowhere/logs", files: [], totalBytes: 0, limitBytes: 52428800, verboseUntilMs: args.on ? Date.now() + 86400000 : 0 });
      if (cmd === "pick_export_target") return Promise.resolve({ token: "t1", path: "/nowhere/x" });
      if (cmd === "save_export") return Promise.resolve();
      return Promise.resolve([]);
    },
  };
  window.__emit = (event, payload) => {
    for (const { handler } of window.__listeners[event] ?? []) if (window[`_${handler}`]) window[`_${handler}`]({ event, id: handler, payload });
  };
}, { p: project, s0: settings(false) });
page.on("pageerror", (e) => console.log("PAGE EXCEPTION:", String(e).slice(0, 300)));
await page.goto(URL, { waitUntil: "networkidle" });
await page.waitForTimeout(500);
await page.locator(".cv-project-open").first().click();
await page.waitForTimeout(800);

const sent = (cmd) => page.evaluate((c) => window.__calls.filter((x) => x.cmd === c), cmd);
const emit = (event, payload) => page.evaluate(([e, p]) => window.__emit(e, p), [event, payload]);
const chat = page.locator('[data-region="ai-chat"]');

// ------------------------------------------------- the button and the shortcut
check("an Ask AI button sits over the workspace", (await page.locator('[data-action="ask-ai"]').count()) === 1);
check("the chat is closed until asked for", (await chat.count()) === 0);
await page.keyboard.press("Control+Shift+A");
await page.waitForTimeout(300);
check("Ctrl+Shift+A opens it", (await chat.count()) === 1);
check("with the chat off it says so and takes no question", (await chat.locator('[data-hint="ai-chat-off"]').count()) === 1 && (await chat.locator('[data-field="ai-chat-input"]').isDisabled()));
await page.keyboard.press("Control+Shift+A");
await page.waitForTimeout(300);
check("and closes it again, to the button", (await chat.count()) === 0 && (await page.locator('[data-action="ask-ai"]').count()) === 1);
check("nothing was sent to a provider while off", (await sent("ai_chat_send")).length === 0 && (await sent("ai_models")).length === 0 && (await sent("ai_test_connection")).length === 0);

// ------------------------------------------------- a turn
await page.evaluate((s) => { window.__settings = s; }, settings(true));
await page.locator('[data-action="ask-ai"]').click();
await page.waitForTimeout(400);
await chat.locator('[data-field="ai-chat-input"]').fill("Is SW1 healthy?");
await chat.locator('[data-field="ai-chat-input"]').press("Enter");
await page.waitForTimeout(200);
const asks = await sent("ai_chat_send");
check("Enter sends the question to Rust, with no context", asks.length === 1 && asks[0].args.text === "Is SW1 healthy?" && asks[0].args.context === null, JSON.stringify(asks));
await emit("corelens://ai-chat", { projectId: "p1", kind: "started" });
await emit("corelens://ai-chat", { projectId: "p1", kind: "text", text: "Checking the ARP " });
await emit("corelens://ai-chat", { projectId: "p1", kind: "text", text: "table." });
await emit("corelens://ai-chat", { projectId: "p1", kind: "toolCall", id: "t1", name: "run_show_command", target: "SW1", command: "show ip arp" });
await page.waitForTimeout(150);
check("Stop is offered while a turn runs", (await chat.locator('[data-action="ai-chat-stop"]').count()) === 1);
await emit("corelens://ai-chat", { projectId: "p1", kind: "toolResult", id: "t1", name: "run_show_command", isError: false, text: "SW1 (192.0.2.10) · Cisco IOS / IOS-XE · `show ip arp`\nParsed with cisco_ios_show_ip_arp.textfsm (1 row(s)):\n{\"ADDRESS\":\"192.0.2.1\",\"INTERFACE\":\"Vlan10\"}\nOutput (2 line(s)):\n..." });
await emit("corelens://ai-chat", { projectId: "p1", kind: "text", text: "SW1 looks healthy." });
await emit("corelens://ai-chat", { projectId: "p1", kind: "done", usage: { inputTokens: 1200, outputTokens: 80 }, session: { inputTokens: 1200, outputTokens: 80 } });
await page.waitForTimeout(250);
const body = await chat.locator('[data-region="ai-chat-body"]').innerText();
check("the answer streams in, in order", /Is SW1 healthy\?[\s\S]*Checking the ARP table\.[\s\S]*run_show_command[\s\S]*SW1 looks healthy\./.test(body), body.slice(0, 400));
const tool = chat.locator('[data-kind="tool"][data-tool="run_show_command"]');
check("the tool call names its target and command", /on SW1.*show ip arp/s.test(await tool.locator("summary").innerText()));
await tool.locator("summary").click();
check("and the rows a template parsed show as a table", (await tool.locator("table td", { hasText: "192.0.2.1" }).count()) === 1 && (await tool.locator("th", { hasText: "INTERFACE" }).count()) === 1);
check("tokens are counted for the answer and the conversation", /this answer 1\.2k in, 80 out.*conversation 1\.2k in, 80 out/s.test(await chat.locator('[data-hint="ai-chat-tokens"]').innerText()), await chat.locator('[data-hint="ai-chat-tokens"]').innerText());

await emit("corelens://ai-chat", { projectId: "p1", kind: "started" });
await page.waitForTimeout(150);
await chat.locator('[data-action="ai-chat-stop"]').click();
await page.waitForTimeout(200);
check("Stop asks Rust to end the turn", (await sent("ai_chat_stop")).length === 1);

// ------------------------------------------------- asking about a device
await page.locator(".react-flow__node", { hasText: "SW1" }).first().click();
await page.waitForTimeout(300);
await page.locator('[data-action="ask-ai-device"]').click();
await page.waitForTimeout(300);
check("the inspector's Ask AI puts the device in front of the question", /About: device SW1 \(192\.0\.2\.10\)/.test(await chat.locator('[data-field="ai-chat-context"]').innerText().catch(() => "")));
await chat.locator('[data-field="ai-chat-input"]').fill("Why is it slow?");
await chat.locator('[data-action="ai-chat-send"]').click();
await page.waitForTimeout(200);
const second = (await sent("ai_chat_send")).at(-1);
check("and the context goes with it", second?.args.context === "device SW1 (192.0.2.10)", JSON.stringify(second));

// ------------------------------------------------- a proposal with a hard block
await emit("corelens://ai-chat", { projectId: "p1", kind: "toolCall", id: "t2", name: "propose_config_change", target: "SW1", command: "interface Vlan10 /  shutdown" });
await page.waitForTimeout(150);
check("the chat shows the proposal waiting, with Open approval", (await chat.locator('[data-hint="ai-chat-awaiting"]').count()) === 1 && (await chat.locator('[data-action="ai-chat-open-approval"]').count()) === 1);
await emit("corelens://change", change("c0ffee01", {}));
await page.waitForTimeout(300);
const dialog = page.locator('[data-region="change-approval"]');
check("a proposal opens the approval dialog by itself", (await dialog.count()) === 1 && (await dialog.getAttribute("data-change")) === "c0ffee01");
const text = await dialog.innerText();
check("it shows the reason, who asked, the commands and the diff", /test the dialog/.test(text) && /Corelens chat \(anthropic, claude-opus-5-5\)/.test(text) && /interface Vlan10\n shutdown/.test(await dialog.locator('[data-field="change-commands"]').innerText()) && /\+ shutdown/.test(await dialog.locator('[data-field="change-diff"]').innerText()));
check("the risk, and the hard block in plain words", /High risk/.test(text) && /carries 192\.0\.2\.10/.test(await dialog.locator('[data-region="change-hard-block"]').innerText()));
check("and a countdown to the deadline", /Counts as denied in \d+ s/.test(await dialog.locator('[data-hint="change-deadline"]').innerText()));
check("the change's id is shown, for get_change_status", /Change c0ffee01, proposed by/.test(await dialog.locator('[data-hint="change-from"]').innerText()));
const approve = dialog.locator('[data-action="change-approve"]');
await dialog.locator('[data-field="change-tick"]').check();
check("ticked but not typed, Approve does nothing", await approve.isDisabled());
await dialog.locator('[data-field="change-typed"]').fill("sw1");
check("the wrong name does not unlock it", await approve.isDisabled());
await dialog.locator('[data-field="change-typed"]').fill("SW1");
check("the exact name does", !(await approve.isDisabled()));
await approve.click();
await page.waitForTimeout(200);
const decided = await sent("change_decide");
check("approving sends the device and the typed name to Rust", decided.length === 1 && JSON.stringify(decided[0].args.approve) === "[0]" && decided[0].args.typed[0] === "SW1", JSON.stringify(decided));

// Rust reports the push, a failed verification, and the rollback.
const failed = change("c0ffee01", { status: "failed", decidedMs: Date.now(), decidedBy: "operator" }, [target({ status: "verifyFailed", message: "Verification failed. The change is still on the device; Roll back puts it back as it was.", verification: [{ command: "show interfaces status", ok: false, output: "% Invalid input" }] })]);
await page.evaluate((c) => { window.__rolledBack = { ...c, status: "failed", targets: c.targets.map((t) => ({ ...t, status: "rolledBack", message: "Rolled back: the running configuration is as it was before the change." })) }; }, failed);
await emit("corelens://change", failed);
await page.waitForTimeout(300);
check("after the decision the dialog follows the change", /verification failed/.test(await dialog.innerText()) && (await dialog.locator('[data-action="change-rollback"]').count()) === 1);
await dialog.locator('[data-action="change-rollback"]').click();
await page.waitForTimeout(300);
check("Roll back asks Rust for that device", JSON.stringify((await sent("change_rollback")).map((c) => [c.args.id, c.args.seq])) === '[["c0ffee01",0]]');
check("and the dialog says it was rolled back", /rolled back/.test(await dialog.innerText()));
await dialog.locator('[data-action="change-close"]').click();
await page.waitForTimeout(200);
check("Close puts the dialog away", (await dialog.count()) === 0);

// ------------------------------------------------- a denial
await emit("corelens://change", change("c0ffee02", {}, [target({ assessment: { class: "low", hardBlocks: [], summary: "Labels and descriptions only." }, commands: ["interface Gi1/0/24", " description LAB-PORT"] })]));
await page.waitForTimeout(300);
await page.locator('[data-region="change-approval"] [data-action="change-deny"]').click();
await page.waitForTimeout(200);
const denied = (await sent("change_decide")).at(-1);
check("Deny sends no device", denied?.args.id === "c0ffee02" && denied.args.approve.length === 0, JSON.stringify(denied));
await emit("corelens://change", change("c0ffee02", { status: "denied", decidedMs: Date.now(), decidedBy: "operator", note: "Denied by the engineer." }, [target({ status: "denied", assessment: { class: "low", hardBlocks: [], summary: "" } })]));
await page.waitForTimeout(250);
check("and the dialog then says it was denied", /denied/.test(await page.locator('[data-region="change-approval"] [data-hint="change-status"]').innerText()));
await page.locator('[data-region="change-approval"] [data-action="change-close"]').click();
await page.waitForTimeout(200);

// ------------------------------------------------- Settings → AI
await page.evaluate((s) => { window.__settings = s; }, settings(false));
// Inside the project, its own: the change record and its chat.
await page.locator(".cv-nav-item", { hasText: /^Settings$/ }).click();
await page.waitForTimeout(500);
const mine = page.locator('[data-region="ai-project"]');
for (const region of ["ai-documents", "ai-mcp-activity", "ai-changes", "ai-project-chat"]) {
  check(`a project's Settings → AI in this project has its ${region} part`, (await mine.locator(`[data-region="${region}"]`).count()) === 1);
}
check("and none of the application's AI settings", (await page.locator('[data-region="ai"]').count()) === 0 && (await page.locator('[data-region="ai-providers"]').count()) === 0);
await mine.locator('[data-action="ai-chat-clear"]').click();
await page.waitForTimeout(200);
check("clearing this project's chat asks for this project only", (await sent("ai_chat_clear")).at(-1)?.args.allProjects === false, JSON.stringify(await sent("ai_chat_clear")));

// On the start screen, the application's.
await page.evaluate(() => window.__cvStore.getState().closeProject());
await page.waitForTimeout(600);
await page.locator('[data-action="app-settings"]').click();
await page.waitForTimeout(500);
const ai = page.locator('[data-region="ai"]');
for (const region of ["ai-chat-settings", "ai-providers", "ai-documents", "ai-mcp", "ai-retention"]) {
  check(`the start screen's Settings → AI has its ${region} part`, (await ai.locator(`[data-region="${region}"]`).count()) === 1);
}
check("and no project's record", (await page.locator('[data-region="ai-changes"]').count()) === 0 && (await page.locator('[data-region="ai-project"]').count()) === 0);
check("the chat is off by default and says nothing is sent", !(await ai.locator('[data-field="ai-chat-enabled"]').isChecked()) && /makes no request to any AI provider/.test(await ai.locator('[data-hint="ai-chat-off-note"]').innerText()));
check("Test connection waits for the chat to be on", await ai.locator('[data-action="ai-test-connection"]').isDisabled());
await ai.locator('[data-action="ai-key-import-anthropic"]').click();
await page.waitForTimeout(200);
check("Paste key asks Rust to read the clipboard, and no key crosses the page", JSON.stringify((await sent("ai_key_import")).map((c) => c.args)) === '[{"provider":"anthropic"}]' && /A key is kept/.test(await ai.locator('[data-hint="ai-key-anthropic"]').innerText()));
check("no model list was asked for while the chat is off", (await sent("ai_models")).length === 0);
// The model is a list that offers every model while one is chosen.
const offered = async (field) => ai.locator(`[data-field="${field}"] option`).evaluateAll((os) => os.map((o) => o.value));
const chatModels = await offered("ai-chat-model");
check("with a model chosen, the chat's model list still offers the others",
  chatModels.includes("claude-opus-5-5") && chatModels.includes("claude-sonnet-5-5") && chatModels.includes("claude-haiku-5-5"), JSON.stringify(chatModels));
await ai.locator('[data-field="ai-chat-model"]').selectOption("claude-sonnet-5-5");
await page.waitForTimeout(200);
check("choosing one keeps it, and the provider's own list shows the same choice",
  (await sent("ai_setting_set")).some((c) => c.args.key === "aiModel.anthropic" && c.args.value === "claude-sonnet-5-5")
  && (await ai.locator('[data-field="ai-model-anthropic"]').inputValue()) === "claude-sonnet-5-5"
  && (await offered("ai-model-anthropic")).includes("claude-opus-5-5"));
await ai.locator('[data-field="ai-model-anthropic"]').selectOption({ label: "Another model ID…" });
await page.waitForTimeout(100);
await ai.locator('[data-field="ai-model-anthropic"]').fill("claude-custom-fixture");
await ai.locator('[data-field="ai-model-anthropic"]').press("Enter");
await page.waitForTimeout(200);
check("a model ID can still be typed by hand, and is then in the list",
  (await sent("ai_setting_set")).some((c) => c.args.key === "aiModel.anthropic" && c.args.value === "claude-custom-fixture")
  && (await offered("ai-model-anthropic")).includes("claude-custom-fixture"));
check("still no model list asked of a provider", (await sent("ai_models")).length === 0);

// Snowflake Cortex: an account, a token through the clipboard, its models.
const sf = ai.locator('[data-provider="snowflake"]');
check("Snowflake Cortex is a provider, with its own card", (await sf.count()) === 1 && (await ai.locator('[data-field="ai-provider"] option[value="snowflake"]').count()) === 1);
await sf.locator('[data-field="ai-snowflake-account"]').fill("myorg-myaccount");
await sf.locator('[data-field="ai-snowflake-account"]').blur();
await page.waitForTimeout(200);
check("the account is kept", (await sent("ai_setting_set")).some((c) => c.args.key === "aiSnowflakeAccount" && c.args.value === "myorg-myaccount"));
await sf.locator('[data-action="ai-key-import-snowflake"]').click();
await page.waitForTimeout(200);
check("its access token comes from the clipboard through Rust, never the page", (await sent("ai_key_import")).some((c) => c.args.provider === "snowflake") && /A key is kept/.test(await ai.locator('[data-hint="ai-key-snowflake"]').innerText()));
await sf.locator('[data-field="ai-snowflake-library"]').fill("EXAMPLE_DB.LIBRARY.PLAN_SEARCH");
await sf.locator('[data-field="ai-snowflake-library"]').blur();
await page.waitForTimeout(200);
check("a plan library is kept, and said to stay in that account", (await sent("ai_setting_set")).some((c) => c.args.key === "aiSnowflakeLibrary" && c.args.value === "EXAMPLE_DB.LIBRARY.PLAN_SEARCH")
  && /plans stay in that account/.test(await sf.innerText()));
check("its model list offers Cortex's Claude models", (await offered("ai-model-snowflake")).includes("claude-sonnet-5-5"));

await ai.locator('[data-field="ai-verbose-logging"]').check();
await page.waitForTimeout(200);
check("verbose logging is a toggle that says when it ends", (await sent("app_log_verbose")).at(-1)?.args.on === true && /On until/.test(await ai.locator('[data-region="ai-retention"]').innerText()));
check("the log's size is shown", /The log holds 0\.0 MB of at most 50 MB/.test(await ai.locator('[data-hint="ai-log-size"]').innerText()));

await browser.close();
console.log(failures === 0 ? "\nall ok" : `\n${failures} failure(s)`);
process.exit(failures === 0 ? 0 : 1);
