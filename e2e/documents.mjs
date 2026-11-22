// The engineer's documents and runbooks, against a stubbed backend that
// plays Rust's answers: the project's folder under its Settings → AI in
// this project, the library under the start screen's Settings → AI (chosen
// and cleared, the documents listed with which
// are runbooks, a PDF beside its Word file, one that could not be read),
// and the chat's Runbooks list (no folder, a runbook's Parts with their
// progress, Start and Continue asking the chat, Start over, nothing sent
// while a turn runs). Invented names and documentation-range addresses only.
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
    nodes: [{ id: "n1", type: "device", position: { x: 260, y: 220 }, data: { label: "EDGE-1", deviceType: "switch", tags: [], addresses: [{ id: "a1", label: "Management", address: "192.0.2.10", isPrimary: true }] } }],
    edges: [], probes: [], canvas: {},
  },
};

const settings = {
  chatEnabled: true, provider: "anthropic",
  models: { anthropic: "claude-opus-5-5", openai: "gpt-6-astra", ollama: "qwen3" },
  openaiBaseUrl: "https://api.openai.com/v1", openaiStyle: "responses", ollamaBaseUrl: "http://localhost:11434",
  maxToolCalls: 15, toolOutputChars: 12000, chatRetentionDays: 90, auditRetentionDays: 90, proposalTimeoutMinutes: 10,
  keys: { anthropic: true, openai: false },
};

const documents = [
  { name: "Edge_Runbook.docx", kind: "docx", project: true, error: null, runbook: "Example Co – Edge swap", copyOf: null, parts: [
    { label: "Start", title: "Start", steps: 2, done: 2, failed: 0, next: null },
    { label: "Part A", title: "Part A – Old edge out", steps: 6, done: 0, failed: 0, next: "A1" },
    { label: "Part B", title: "Part B – Routing", steps: 3, done: 1, failed: 1, next: "B2" },
  ] },
  { name: "design/LLD.docx", kind: "docx", project: true, error: null, runbook: null, copyOf: null, parts: [] },
  { name: "design/LLD.pdf", kind: "pdf", project: true, error: null, runbook: null, copyOf: "LLD.docx", parts: [] },
  { name: "Vendor guide.pdf", kind: "pdf", project: false, error: "no text in it (a scan, perhaps)", runbook: null, copyOf: null, parts: [] },
];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
await page.addInitScript(({ p, s0, docs }) => {
  window.__calls = [];
  window.__folders = { project: null, library: null };
  window.__docs = docs;
  window.__listeners = {};
  let next = 1;
  const status = () => ({ folders: window.__folders, projectOpen: true, documents: window.__folders.project || window.__folders.library ? window.__docs : [] });
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
      if (cmd === "app_info") return Promise.resolve({ version: "2.18.0", dataDir: "/nowhere", documentVersion: 1 });
      if (cmd === "mcp_status") return Promise.resolve({ enabled: false, listening: false, error: null, executable: "x", claudeCodeCommand: "x", claudeDesktopConfig: "{}" });
      if (cmd === "ai_settings") return Promise.resolve(s0);
      if (cmd === "ai_chat_history") return Promise.resolve({ rows: [], session: { inputTokens: 0, outputTokens: 0 }, running: false });
      if (cmd === "ai_chat_send") return Promise.resolve(null);
      if (cmd === "app_log_info") return Promise.resolve({ folder: "/nowhere/logs", files: [], totalBytes: 0, limitBytes: 52428800, verboseUntilMs: 0 });
      if (cmd === "plugin:dialog|open") return Promise.resolve(window.__pick ?? null);
      if (cmd === "library_status") return Promise.resolve(status());
      if (cmd === "library_folder_set") { window.__folders = { ...window.__folders, [args.scope]: args.path }; return Promise.resolve(status()); }
      if (cmd === "runbook_step_set") {
        window.__docs = window.__docs.map((d) => d.name !== args.document ? d : { ...d, parts: d.parts.map((q) => q.label !== args.part ? q : { ...q, done: 0, failed: 0, next: "B1" }) });
        return Promise.resolve(status());
      }
      return Promise.resolve([]);
    },
  };
  window.__emit = (event, payload) => {
    for (const { handler } of window.__listeners[event] ?? []) if (window[`_${handler}`]) window[`_${handler}`]({ event, id: handler, payload });
  };
}, { p: project, s0: settings, docs: documents });
page.on("pageerror", (e) => console.log("PAGE EXCEPTION:", String(e).slice(0, 300)));
await page.goto(URL, { waitUntil: "networkidle" });
await page.waitForTimeout(500);
await page.locator(".cv-project-open").first().click();
await page.waitForTimeout(800);

const sent = (cmd) => page.evaluate((c) => window.__calls.filter((x) => x.cmd === c), cmd);
const emit = (event, payload) => page.evaluate(([e, p]) => window.__emit(e, p), [event, payload]);

// ------------------------------------------------- the chat's Runbooks, before a folder
await page.locator('[data-action="ask-ai"]').click();
await page.waitForTimeout(400);
const chat = page.locator('[data-region="ai-chat"]');
check("the chat has a Runbooks button", (await chat.locator('[data-region="ai-chat-runbooks"]').count()) === 1);
await chat.locator('[data-region="ai-chat-runbooks"]').click();
await page.waitForTimeout(300);
check("with no folder it says where to choose one", /Settings → AI in this project/.test(await chat.locator('[data-hint="runbooks-no-folder"]').innerText().catch(() => "")));
await chat.locator('[data-region="ai-chat-collapse"]').click();
await page.waitForTimeout(200);

// ------------------------------------------------- Settings → AI → Documents
await page.locator(".cv-nav-item", { hasText: /^Settings$/ }).click();
await page.waitForTimeout(500);
const docs = page.locator('[data-region="ai-documents"]');
check("a project's Settings → AI in this project has a Documents part", (await docs.count()) === 1 && (await page.locator('[data-region="ai-project"] [data-region="ai-documents"]').count()) === 1);
check("with this project's folder only: the library is the application's", (await docs.locator('[data-region="docs-folder-library"]').count()) === 0);
check("it says what is sent and that nothing is uploaded", /Only the sections a search returns are sent/.test(await docs.innerText()) && /never writes to these folders/.test(await docs.innerText()));
check("the folder is not set at first", (await docs.locator('[data-field="docs-path-project"]').innerText()) === "Not set");
check("Clear waits for a folder", await docs.locator('[data-action="docs-clear-project"]').isDisabled());
await page.evaluate(() => { window.__pick = "/work/example/docs"; });
await docs.locator('[data-action="docs-choose-project"]').click();
await page.waitForTimeout(300);
const set = await sent("library_folder_set");
check("choosing the project's folder sends it to Rust for this project", set.length === 1 && set[0].args.scope === "project" && set[0].args.path === "/work/example/docs", JSON.stringify(set));
check("and the folder is shown", (await docs.locator('[data-field="docs-path-project"]').innerText()) === "/work/example/docs");
const list = docs.locator('[data-region="docs-list"]');
check("this project's documents are listed, with how many", (await list.locator("li").count()) === 3 && /3 documents/.test(await docs.locator('[data-hint="docs-count"]').innerText()));
check("a runbook says so, with its Parts", /Runbook, 3 Parts/.test(await list.locator('[data-doc="Edge_Runbook.docx"]').innerText()) && (await list.locator('[data-kind="runbook"]').count()) === 1);
check("a PDF beside its Word file says which one is read", /The same as LLD\.docx beside it/.test(await list.locator('[data-doc="design/LLD.pdf"]').innerText()));
check("and the library's are not", (await list.locator('[data-doc="Vendor guide.pdf"]').count()) === 0 && /project/.test(await list.locator('[data-doc="design/LLD.docx"]').innerText()));
const before = (await sent("library_status")).length;
await docs.locator('[data-action="docs-refresh"]').click();
await page.waitForTimeout(200);
check("Read again asks Rust to read the folders again", (await sent("library_status")).length === before + 1);

// ------------------------------------------------- the chat's Runbooks
await page.locator('[data-action="ask-ai"]').click();
await page.waitForTimeout(400);
const books = chat.locator('[data-region="ai-runbooks"]');
check("the Runbooks list stays open, and lists the runbook", (await books.locator('[data-runbook="Edge_Runbook.docx"]').count()) === 1 && /Example Co – Edge swap/.test(await books.innerText()));
check("only runbooks are listed", (await books.locator("[data-runbook]").count()) === 1);
const partA = books.locator('[data-part="Part A"]');
const partB = books.locator('[data-part="Part B"]');
const start = books.locator('[data-part="Start"]');
check("each Part shows how far it got", /0 of 6 done/.test(await partA.innerText()) && /1 of 3 done · 1 failed/.test(await partB.innerText()));
check("a finished Part is marked done and offers no Start", (await start.getAttribute("class")).includes("is-done") && (await start.locator('[data-action="runbook-start"], [data-action="runbook-continue"]').count()) === 0);
check("a Part with a failed step is marked", (await partB.getAttribute("class")).includes("is-failed"));
await partA.locator('[data-action="runbook-start"]').click();
await page.waitForTimeout(200);
const asked = await sent("ai_chat_send");
check("Start asks the chat to start that Part of that runbook", asked.length === 1 && asked[0].args.text === "Start Part A of the runbook Edge_Runbook.docx.", JSON.stringify(asked));
check("the question shows in the conversation", /Start Part A of the runbook Edge_Runbook\.docx\./.test(await chat.locator('[data-region="ai-chat-body"]').innerText()));
await emit("corelens://ai-chat", { projectId: "p1", kind: "started" });
await page.waitForTimeout(150);
check("nothing more can be started while a turn runs", await partB.locator('[data-action="runbook-continue"]').isDisabled());
await emit("corelens://ai-chat", { projectId: "p1", kind: "done", usage: { inputTokens: 10, outputTokens: 5 }, session: { inputTokens: 10, outputTokens: 5 } });
await page.waitForTimeout(300);
await partB.locator('[data-action="runbook-continue"]').click();
await page.waitForTimeout(200);
const again = (await sent("ai_chat_send")).at(-1);
check("Continue asks from the next step not done", again?.args.text === "Continue Part B of the runbook Edge_Runbook.docx, from step B2.", JSON.stringify(again));
await emit("corelens://ai-chat", { projectId: "p1", kind: "done", usage: { inputTokens: 1, outputTokens: 1 }, session: { inputTokens: 11, outputTokens: 6 } });
await page.waitForTimeout(300);
await partB.locator('[data-action="runbook-reset"]').click();
await page.waitForTimeout(300);
const reset = await sent("runbook_step_set");
check("Start over clears that Part's record in Rust", reset.length === 1 && reset[0].args.document === "Edge_Runbook.docx" && reset[0].args.part === "Part B" && reset[0].args.step === null && reset[0].args.status === "pending", JSON.stringify(reset));
check("and the Part is back to its first step", /0 of 3 done/.test(await partB.innerText()) && (await partB.locator('[data-action="runbook-start"]').count()) === 1);
check("the list is read again when a turn ends, since the chat records steps", (await sent("library_status")).length >= before + 3);

// ------------------------------------------------- the library, on the start screen
await page.evaluate(() => window.__cvStore.getState().closeProject());
await page.waitForTimeout(600);
await page.locator('[data-action="app-settings"]').click();
await page.waitForTimeout(500);
const lib = page.locator('[data-region="ai"] [data-region="ai-documents"]');
check("the start screen's Settings → AI has the library folder, and no project's", (await lib.locator('[data-region="docs-folder-library"]').count()) === 1 && (await lib.locator('[data-region="docs-folder-project"]').count()) === 0);
const chosen = (await sent("library_folder_set")).length;
await page.evaluate(() => { window.__pick = null; });
await lib.locator('[data-action="docs-choose-library"]').click();
await page.waitForTimeout(300);
check("a cancelled choice changes nothing", (await sent("library_folder_set")).length === chosen);
await page.evaluate(() => { window.__pick = "/work/library"; });
await lib.locator('[data-action="docs-choose-library"]').click();
await page.waitForTimeout(300);
const libSet = (await sent("library_folder_set")).at(-1);
check("choosing it sends the library's folder to Rust", libSet?.args.scope === "library" && libSet?.args.path === "/work/library", JSON.stringify(libSet));
check("and only the library's documents are listed there, with why one could not be read", (await lib.locator('[data-region="docs-list"] li').count()) === 1 && /Could not be read: no text in it/.test(await lib.locator('[data-doc="Vendor guide.pdf"]').innerText()) && /library/.test(await lib.locator('[data-doc="Vendor guide.pdf"]').innerText()));

await browser.close();
console.log(failures ? `\n${failures} check(s) failed` : "\nall documents checks passed");
process.exit(failures ? 1 : 0);
