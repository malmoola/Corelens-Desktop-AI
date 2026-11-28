// What is selected is what the canvas shows as selected, however it came to
// be: a click, a box, Ctrl+click, a right-click. Delete and the menu's
// Delete remove that and nothing else, and the inspector is about it.
//
//     npm run dev            # in another terminal
//     node e2e/selection.mjs
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
  meta: { id: "sel", name: "Selection", customer: "", site: "", createdAt: NOW, updatedAt: NOW },
  document: { activePageId: "p1", probes: [], pages: [{ id: "p1", name: "Core", canvas,
    nodes: [dev("x", "SW-X", 0, 0, "192.0.2.9"), dev("a", "SW-A", 320, 0, "192.0.2.1"), dev("b", "SW-B", 520, 0, "192.0.2.2"), dev("c", "SW-C", 720, 0, "192.0.2.3"), dev("d", "SW-D", 320, 360, "192.0.2.4")],
    edges: [{ id: "e1", source: "x", target: "d", sourceHandle: "b", targetHandle: "l", type: "live",
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


const picture = () => st(() => {
  const s = window.__cvStore.getState(), pg = s.doc.pages[0];
  return { marked: pg.nodes.filter((n) => n.selected).map((n) => n.id).sort().join(","), named: s.selectedNodeId ?? s.selectedEdgeId ?? "" };
});
const box = (id) => page.locator(`.react-flow__node[data-id="${id}"]`).boundingBox();
/** A selection box drawn from above-left of one device to below-right of another. */
const drawBox = async (from, to) => {
  const a = await box(from), b = await box(to);
  await page.mouse.move(a.x - 30, a.y - 30);
  await page.mouse.down();
  await page.mouse.move(a.x, a.y, { steps: 3 });
  await page.mouse.move(b.x + b.width + 30, b.y + b.height + 30, { steps: 8 });
  await page.mouse.up();
  await settle();
};
/** Several devices selected one by one, the first by a click and the rest
 *  with Ctrl held: each stays a device to be clicked, where a box leaves
 *  its own frame over what it caught. */
const pick = async (...labels) => {
  await named(labels[0]).click();
  await page.keyboard.down("Control");
  for (const label of labels.slice(1)) await named(label).click();
  await page.keyboard.up("Control");
  await settle();
};
const reset = async () => {
  await st((p) => { const s = window.__cvStore.getState(); s.selectNone(); window.__cvStore.setState((x) => ({ doc: { ...x.doc, probes: [], pages: [{ ...x.doc.pages[0], nodes: p.nodes.map((n) => ({ ...n })), edges: p.edges.map((e) => ({ ...e })) }] } })); }, project.document.pages[0]);
  await settle();
};

// --------------------------------- a click, then a box round other devices
await named("SW-X").click();
await settle();
check("(a click selects one device and names it)", JSON.stringify(await picture()) === JSON.stringify({ marked: "x", named: "x" }), JSON.stringify(await picture()));
await drawBox("a", "b");
{
  const now = await picture();
  check("a box round two other devices selects those two, and names nobody", now.marked === "a,b" && now.named === "", JSON.stringify(now));
  check("the inspector says two devices", /2 (devices|objects)/i.test(await page.locator(".cv-inspector").innerText()), (await page.locator(".cv-inspector").innerText()).slice(0, 60));
  await page.keyboard.press("Delete");
  await settle();
  check("Delete removes the two in the box, not the one clicked before", (await devices()) === "c,d,x", await devices());
}
await reset();

// ----------------------------------------------- a box round a single device
await named("SW-X").click();
await settle();
await drawBox("c", "c");
{
  const now = await picture();
  check("a box round one device makes it the one the inspector shows", now.marked === "c" && now.named === "c", JSON.stringify(now));
  const title = await page.locator(".cv-inspector").innerText();
  check("and the inspector is about it, not the one clicked before", title.includes("SW-C") && !title.includes("SW-X"), title.slice(0, 60));
}
await reset();

// -------------------------------- Ctrl+click takes one out of the selection
await pick("SW-A", "SW-B");
check("(two devices selected with Ctrl+click)", (await picture()).marked === "a,b", JSON.stringify(await picture()));
await page.keyboard.down("Control");
await named("SW-B").click();
await page.keyboard.up("Control");
await settle();
{
  const now = await picture();
  check("Ctrl+click on a selected device takes it out, and leaves the other named", now.marked === "a" && now.named === "a", JSON.stringify(now));
  await page.keyboard.press("Delete");
  await settle();
  check("Delete then removes only what is still selected", (await devices()) === "b,c,d,x", await devices());
}
await reset();

// ---------------------------------------- right-click on an unselected object
await pick("SW-A", "SW-B", "SW-C");
check("(three devices selected)", (await picture()).marked === "a,b,c", JSON.stringify(await picture()));
await named("SW-D").click({ button: "right" });
await settle();
check("a right-click on a device outside the selection selects it alone", (await picture()).marked === "d", JSON.stringify(await picture()));
await page.locator(".cv-menu button", { hasText: /^Delete$/ }).click();
await settle();
check("and its Delete removes that device only", (await devices()) === "a,b,c,x", await devices());
await reset();

// ------------------------------------- right-click on one of several selected
await pick("SW-A", "SW-B", "SW-C");
await named("SW-B").click({ button: "right" });
await settle();
check("a right-click on one of several selected keeps them all", (await picture()).marked === "a,b,c", JSON.stringify(await picture()));
await page.locator(".cv-menu button", { hasText: /^Delete$/ }).click();
await settle();
check("and its Delete removes the selection", (await devices()) === "d,x", await devices());
await page.keyboard.press("Control+z");
await settle();
check("one undo brings them back", (await devices()) === "a,b,c,d,x", await devices());
await reset();

// ----------------------------- right-click on the frame a box leaves behind
await drawBox("a", "c");
{
  const frame = await page.locator(".react-flow__nodesselection-rect").boundingBox();
  check("(a box leaves its frame round what it caught)", frame !== null && (await picture()).marked === "a,b,c", JSON.stringify(await picture()));
  if (frame) await page.mouse.click(frame.x + frame.width / 2, frame.y + 6, { button: "right" });
  await settle();
  const items = await page.locator(".cv-menu button").allInnerTexts();
  check("a right-click on that frame opens the selection's menu", items.includes("Delete"), items.slice(0, 6).join(", "));
  await page.locator(".cv-menu button", { hasText: /^Delete$/ }).click().catch(() => {});
  await settle();
  check("and its Delete removes what the box caught", (await devices()) === "d,x", await devices());
}
await reset();

// ------------------------------------------------ a link, with devices selected
await pick("SW-A", "SW-B");
{
  const at = await st(() => {
    const path = document.querySelector('.react-flow__edge[data-id="e1"] path.react-flow__edge-path');
    const p = path.getPointAtLength(path.getTotalLength() / 2).matrixTransform(path.getScreenCTM());
    return { x: p.x, y: p.y };
  });
  await page.mouse.click(at.x, at.y, { button: "right" });
  await settle();
  const now = await st(() => { const pg = window.__cvStore.getState().doc.pages[0]; return { nodes: pg.nodes.filter((n) => n.selected).map((n) => n.id).join(","), edges: pg.edges.filter((e) => e.selected).map((e) => e.id).join(",") }; });
  check("a right-click on a link selects the link alone", now.nodes === "" && now.edges === "e1", JSON.stringify(now));
  await page.locator(".cv-menu button", { hasText: /^Delete$/ }).click();
  await settle();
  const left = await st(() => { const pg = window.__cvStore.getState().doc.pages[0]; return `${pg.nodes.length} devices, ${pg.edges.length} links`; });
  check("and its Delete removes the link, not the devices that were selected", left === "5 devices, 0 links", left);
}

// ----------------------------------------------- choosing is not unsaved work
await reset();
await st(() => window.__cvStore.setState({ dirty: false }));
await named("SW-A").click();
await drawBox("a", "c");
await settle();
check("selecting devices and links does not make the project unsaved", (await st(() => window.__cvStore.getState().dirty)) === false);

await browser.close();
console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
