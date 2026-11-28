// The diagram's keys act on the diagram only while it is what is in use:
// not from another screen with the diagram hidden behind it, not while a
// menu or a dialog is open over it, and not instead of copying text that
// was selected to be copied.
//
//     npm run dev            # in another terminal
//     node e2e/keysscope.mjs
import { chromium } from "playwright";

const URL = process.env.CV_URL ?? "http://localhost:5173/";
const NOW = 1756000000000;

let failures = 0;
const check = (name, ok, detail = "") => {
  if (ok) console.log(`ok   ${name}`);
  else { failures++; console.log(`FAIL ${name}${detail ? `  ${detail}` : ""}`); }
};

const canvas = { gridEnabled: false, snapEnabled: false, minimap: false, nodeStyle: "glyph" };
const dev = (id, label, x, y, address) => ({ id, type: "device", position: { x, y }, width: 76, height: 76,
  data: { label, deviceType: "access-switch", tags: [], addresses: [{ id: `${id}-a`, label: "Management", address, isPrimary: true }], locked: false, maintenance: false, showDetails: true } });
const project = {
  meta: { id: "keys", name: "Keys", customer: "", site: "", createdAt: NOW, updatedAt: NOW },
  document: { activePageId: "p1", probes: [], pages: [{ id: "p1", name: "Core", canvas,
    nodes: [dev("a", "SW-A", 0, 0, "192.0.2.1"), dev("b", "SW-B", 300, 0, "192.0.2.2"), dev("c", "SW-C", 150, 260, "192.0.2.3")],
    edges: [{ id: "e1", source: "a", target: "b", sourceHandle: "r", targetHandle: "l", type: "live",
      data: { sourcePortLabel: "", targetPortLabel: "", label: "", direction: "none", enabled: true, maintenance: false, pathType: "straight", healthRule: { type: "manual" } } }] }] },
};

const browser = await chromium.launch();
const page = await (await browser.newContext({ viewport: { width: 1500, height: 950 } })).newPage();
await page.addInitScript(({ p }) => {
  let next = 1;
  window.__saves = 0;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    transformCallback() { return next++; },
    invoke(cmd) {
      if (cmd === "plugin:event|listen") return Promise.resolve(next++);
      const meta = { id: p.meta.id, name: p.meta.name, customer: "", site: "", ticket: "", engineer: "", description: "", created_at: p.meta.createdAt, updated_at: p.meta.updatedAt, archived: false };
      if (cmd === "list_projects") return Promise.resolve([meta]);
      if (cmd === "load_project") return Promise.resolve({ meta, document_version: 1, document: p.document });
      if (cmd === "get_settings") return Promise.resolve({});
      if (cmd === "vault_status") return Promise.resolve({ exists: true, unlocked: true, credentials: 0, minimumPassphrase: 12 });
      if (cmd === "save_project") { window.__saves += 1; return Promise.resolve(null); }
      if (cmd === "describe_subnet") return Promise.resolve(null);
      return Promise.resolve([]);
    },
  };
}, { p: project });
page.on("pageerror", (e) => console.log("PAGE EXCEPTION:", String(e).slice(0, 300)));
await page.goto(URL, { waitUntil: "networkidle" });
await page.locator(".cv-project-open").first().click();
await page.waitForTimeout(900);

const st = (fn, arg) => page.evaluate(fn, arg);
const settle = () => page.waitForTimeout(250);
const devices = () => st(() => window.__cvStore.getState().doc.pages[0].nodes.map((n) => n.id).sort().join(","));
const where = (id) => st((id) => { const n = window.__cvStore.getState().doc.pages[0].nodes.find((x) => x.id === id); return n ? `${Math.round(n.position.x)},${Math.round(n.position.y)}` : "gone"; }, id);
const selected = () => st(() => window.__cvStore.getState().doc.pages[0].nodes.filter((n) => n.selected).map((n) => n.id).sort().join(","));
const named = (label) => page.locator(".react-flow__node", { hasText: label }).first();
const rail = (label) => page.locator(".cv-nav-item", { hasText: new RegExp(`^${label}$`) });
// The keyboard on nothing in particular: where it is after a click on a
// part of the screen that is not a control.
const offEverything = () => st(() => { if (document.activeElement instanceof HTMLElement) document.activeElement.blur(); });

await named("SW-A").click();
await settle();
check("(the fixture: three devices, one of them selected)", (await devices()) === "a,b,c" && (await selected()) === "a", `${await devices()} / ${await selected()}`);

// ------------------------------------- another screen, the diagram behind it
for (const screen of ["Addresses", "Racks", "Inventory"]) {
  await rail(screen).click();
  await settle();
  const hidden = await st(() => { const el = document.querySelector(".cv-canvas"); return !el || el.getBoundingClientRect().width === 0; });
  await offEverything();
  await page.keyboard.press("Delete");
  await page.keyboard.press("Backspace");
  await settle();
  check(`on ${screen}, with the diagram behind it, Delete removes nothing from the diagram`, hidden && (await devices()) === "a,b,c", `hidden=${hidden} ${await devices()}`);
  const before = await where("a");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Shift+ArrowDown");
  await settle();
  check(`and the arrow keys move nothing on it`, (await where("a")) === before, `${before} -> ${await where("a")}`);
  await page.keyboard.press("Control+a");
  await settle();
  check(`nor does Ctrl+A select what cannot be seen`, (await selected()) === "a", await selected());
}
// What is the project's stays the project's on any screen: save, and undo.
{
  const saves = await st(() => window.__saves);
  await st(() => window.__cvStore.setState({ dirty: true }));
  await page.keyboard.press("Control+s");
  await settle();
  check("Ctrl+S still saves the project from another screen", (await st(() => window.__saves)) === saves + 1, `${saves} -> ${await st(() => window.__saves)}`);
}
await rail("Diagram").click();
await settle();

// ------------------------------------------------------ a menu open over it
{
  await named("SW-A").click();
  await settle();
  const before = await where("a");
  await named("SW-A").click({ button: "right" });
  await settle();
  const open = (await page.locator(".cv-menu").count()) === 1;
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  await settle();
  check("with a menu open, the arrow keys do not move the device under it", open && (await where("a")) === before, `open=${open} ${before} -> ${await where("a")}`);
  await page.keyboard.press("Delete");
  await settle();
  check("and Delete does not remove it", (await devices()) === "a,b,c", await devices());
  await page.keyboard.press("Escape");
  await settle();
}

// ---------------------------------------------------- a dialog open over it
{
  await named("SW-B").click();
  await settle();
  // The keyboard shortcuts' own dialog, opened and then the keyboard put
  // on its close button, as a click there would.
  await page.keyboard.press("?");
  await settle();
  const dialog = page.locator('[role="dialog"]').first();
  const shown = (await dialog.count()) === 1;
  await dialog.locator("button").first().focus().catch(() => {});
  await page.keyboard.press("Delete");
  await settle();
  check("with a dialog open, Delete does not remove the selection behind it", shown && (await devices()) === "a,b,c", `shown=${shown} ${await devices()}`);
  await page.keyboard.press("Escape");
  await settle();
}

// --------------------------------------------- text selected to be copied
{
  await named("SW-C").click();
  await settle();
  await st(() => window.__cvStore.getState().setStatusMessage(""));
  // Some words of the inspector's, selected as a drag across them would.
  const picked = await st(() => {
    const el = document.querySelector(".cv-inspector .cv-help, .cv-inspector h2, .cv-inspector h3");
    if (!el) return "";
    const range = document.createRange();
    range.selectNodeContents(el);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    return sel.toString();
  });
  // Whether the key was taken from the browser, which is what stops the
  // text reaching the clipboard. Armed first, read after the key.
  const arm = () => st(() => {
    window.__taken = undefined;
    const seen = (e) => {
      if (e.key.toLowerCase() !== "c") return;
      window.removeEventListener("keydown", seen, true);
      setTimeout(() => { window.__taken = e.defaultPrevented; }, 0);
    };
    window.addEventListener("keydown", seen, true);
  });
  await arm();
  await page.keyboard.press("Control+c");
  await settle();
  const taken = await st(() => window.__taken);
  const said = await st(() => window.__cvStore.getState().statusMessage ?? "");
  check("Ctrl+C with text selected copies the text, not the selected device", picked.length > 0 && taken === false && !/^Copied/.test(said), `picked=${JSON.stringify(picked.slice(0, 30))} prevented=${taken} said=${JSON.stringify(said)}`);
  await st(() => window.getSelection().removeAllRanges());
  // And with no text selected it copies the device, as it always did.
  await arm();
  await page.keyboard.press("Control+c");
  await settle();
  const tookDevice = await st(() => window.__taken);
  check("with no text selected it copies the device", tookDevice === true && /^Copied 1 object/.test(await st(() => window.__cvStore.getState().statusMessage ?? "")), `prevented=${tookDevice} ${await st(() => window.__cvStore.getState().statusMessage ?? "")}`);
}

// ----------------------------------------------- the diagram itself, as ever
{
  await named("SW-C").click();
  await settle();
  await offEverything();
  await page.keyboard.press("Delete");
  await settle();
  check("on the diagram, Delete removes the selected device", (await devices()) === "a,b", await devices());
  await page.keyboard.press("Control+z");
  await settle();
  check("and Ctrl+Z brings it back", (await devices()) === "a,b,c", await devices());
  await named("SW-C").click();
  await settle();
  const before = await where("c");
  await page.keyboard.press("ArrowRight");
  await settle();
  check("the arrow keys nudge it", (await where("c")) !== before, `${before} -> ${await where("c")}`);
  // Undo from another screen takes back what was done on the diagram.
  await offEverything();
  await page.keyboard.press("Delete");
  await settle();
  const without = await devices();
  await rail("Addresses").click();
  await settle();
  await offEverything();
  await page.keyboard.press("Control+z");
  await settle();
  check("Ctrl+Z still undoes from another screen", without === "a,b" && (await devices()) === "a,b,c", `${without} -> ${await devices()}`);
  await rail("Diagram").click();
  await settle();
}

await browser.close();
console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
