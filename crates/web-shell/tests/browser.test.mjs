import { readFileSync } from "node:fs";
import test from "node:test";
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
const core = await import(pathToFileURL(process.env.CORE_MODULE));
const { FragmentController, copyTarget, pointers, resolveKey } = await import(pathToFileURL(process.env.SHELL_MODULE));

test("stale-reply-never-overwrites even when transport ignores cancellation", async () => {
  const requests = new core.LatestRequest();
  const displayed = [];
  let oldReply;
  let oldSignal;
  const first = requests.run("inspector", signal => { oldSignal = signal; return new Promise(resolve => { oldReply = resolve; }); }, value => displayed.push(value));
  const second = requests.run("inspector", async () => "new", value => displayed.push(value));
  assert.equal(await second, true);
  assert.equal(oldSignal.aborted, true);
  oldReply("old");
  assert.equal(await first, false);
  assert.deepEqual(displayed, ["new"]);
});
test("hidden tab aborts in-flight results and independent targets do not supersede", async () => {
  const requests = new core.LatestRequest();
  const shown = [];
  let finish;
  const pending = requests.run("reader", () => new Promise(resolve => { finish = resolve; }), value => shown.push(value));
  await requests.run("inspector", async () => "facts", value => shown.push(value));
  requests.abortAll(); finish("hidden"); await pending;
  assert.deepEqual(shown, ["facts"]);
});
test("denied-clipboard-is-honest and character count uses Unicode characters", async () => {
  assert.deepEqual(await core.copyText("A🦋", { writeText: async () => { throw new Error("denied"); } }), { copied: false, characters: 2 });
  assert.deepEqual(await core.copyText("abc", undefined), { copied: false, characters: 3 });
  let copied;
  assert.deepEqual(await core.copyText("A🦋", { writeText: async value => { copied = value; } }), { copied: true, characters: 2 });
  assert.equal(copied, "A🦋");
});
test("append is held outside follow-at-end and protected replacement always waits", () => {
  for (const following of [false, true]) for (const atEnd of [false, true]) for (const protectedRow of [false, true]) {
    const state = { following, atEnd, protected: protectedRow };
    assert.equal(core.shouldHold("append", state), protectedRow || !following || !atEnd);
    assert.equal(core.shouldHold("replace", state), protectedRow);
  }
  assert.equal(core.nearEnd(50, 50, 101), true);
  assert.equal(core.nearEnd(45, 50, 101), false);
});
test("pointer zoom fixes the time under the pointer and pan preserves span", () => {
  const zoom = core.pointerRange(100, 200, 400, 0, 2, .25);
  assert.deepEqual(zoom, [112.5, 162.5]);
  assert.equal(zoom[0] + .25 * (zoom[1] - zoom[0]), 125);
  assert.deepEqual(core.pointerRange(100, 200, 400, 40, 1, .5), [90, 190]);
});
test("palette matcher preserves letters and ranks a word boundary above an interior match", () => {
  assert.equal(core.matchScore("zz", "Read result"), null);
  assert.ok(core.matchScore("rr", "Read result") > core.matchScore("rr", "arbitrary result"));
  assert.notEqual(core.matchScore("CLI", "copy as cli command"), null);
});
test("operation URLs reject foreign origins and active URL schemes", () => {
  assert.equal(core.sameOrigin("/v1/read", "https://example.test/home").pathname, "/v1/read");
  assert.throws(() => core.sameOrigin("https://other.test/", "https://example.test/"));
  assert.throws(() => core.sameOrigin("javascript:alert(1)", "https://example.test/"));
});

// Minimal DOM port: the assertions concern mutations, focus and scroll, not source text.
class Element {
  constructor(id = "") { this.id = id; this.dataset = {}; this.children = []; this.html = "old"; this.scrollTop = 17; this.clientHeight = 100; this.scrollHeight = 300; this.attributes = new Map(); this.listeners = new Map(); this.events = []; this.scrollLeft = 0; }
  closest(selector) { if (selector === "[data-follow]" && "follow" in this.dataset) return this; return null; }
  contains(element) { return element?.parent === this; }
  querySelector(selector) { if (selector.includes("aria-selected")) return this.selected ?? null; return null; }
  querySelectorAll() { return []; }
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  dispatchEvent(event) { this.events.push(event); return true; }
  setPointerCapture(id) { this.capture = id; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 400, bottom: 300 }; }
  setAttribute(name, value) { this.attributes.set(name, value); }
  focus() { document.activeElement = this; }
  select() { this.textSelected = true; }
  after(element) { document.notices.push(element); }
  replaceChildren(fragment) { this.html = fragment.html; this.scrollHeight += 25; }
  append(fragment) { if (fragment instanceof Element) this.children.push(fragment); else { this.html += fragment.html; this.scrollHeight += 25; } }
  remove() { document.notices = document.notices.filter(element => element !== this); }
}
function dom() {
  const reader = new Element("reader"); reader.dataset.follow = "false";
  const doc = {
    notices: [], body: new Element("body"), activeElement: null,
    getElementById(id) { return id === "reader" ? reader : null; },
    getSelection() { return { toString: () => "", rangeCount: 0 }; },
    querySelectorAll(selector) { return selector === "[data-updates]" ? this.notices : []; },
    createElement(tag) {
      if (tag === "template") {
        const content = { html: "", querySelector: () => null, querySelectorAll: () => [] };
        return { content, set innerHTML(value) { content.html = value; } };
      }
      return new Element();
    }
  };
  globalThis.HTMLElement = Element; globalThis.document = doc;
  return { doc, reader, controller: new FragmentController(doc) };
}
test("update-never-moves-reader: append leaves DOM, scroll and focus unchanged until apply", () => {
  const { doc, reader, controller } = dom(); const focus = new Element("focused"); focus.parent = reader; doc.activeElement = focus;
  controller.receive({ kind: "append", target: "reader", html: "new" });
  assert.equal(reader.html, "old"); assert.equal(reader.scrollTop, 17); assert.equal(doc.activeElement, focus);
  assert.equal(doc.notices[0].textContent, "1 new ↓");
  controller.flush("reader"); assert.equal(reader.html, "old");
  doc.activeElement = doc.body; controller.flush("reader");
  assert.equal(reader.html, "oldnew"); assert.equal(reader.scrollTop, reader.scrollHeight); assert.equal(doc.notices.length, 0);
});
test("selected rows survive replacement; newest queued replacement wins", () => {
  const { doc, reader, controller } = dom(); reader.selected = {};
  controller.receive({ kind: "replace", target: "reader", html: "first" });
  controller.receive({ kind: "replace", target: "reader", html: "second" });
  assert.equal(reader.html, "old"); assert.equal(reader.scrollTop, 17);
  delete reader.selected; doc.activeElement = doc.body; controller.flush("reader");
  assert.equal(reader.html, "second"); assert.equal(reader.scrollTop, 17);
});
test("live follow-at-end appends without taking focus", () => {
  const { doc, reader, controller } = dom(); reader.dataset.follow = "true"; reader.scrollTop = 200; doc.activeElement = doc.body;
  controller.receive({ kind: "append", target: "reader", html: "new" });
  assert.equal(reader.html, "oldnew"); assert.equal(reader.scrollTop, reader.scrollHeight); assert.equal(doc.activeElement, doc.body);
});

test("palette and terminal picker run the same independent matcher vectors", () => {
  const vectors = JSON.parse(readFileSync(new URL("matcher.json", import.meta.url), "utf8"));
  for (const vector of vectors) assert.equal(core.matchScore(vector.query, vector.label), vector.score, JSON.stringify(vector));
});

test("clipboard denial exposes selected text beside target and never reports copied", async () => {
  const { doc } = dom();
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async () => { throw new Error("denied"); } } } });
  const source = new Element("copy"); source.dataset.copy = JSON.stringify({ label: "message", forms: [{ label: "Markdown", text: "A🦋" }] });
  const feedback = new Element(); source.querySelector = selector => selector === "[data-copy-feedback]" ? feedback : null;
  await copyTarget(source, 0);
  assert.equal(feedback.textContent, "Clipboard unavailable. Select and copy the text below.");
  assert.equal(source.children[0].value, "A🦋");
  assert.equal(source.children[0].textSelected, true); assert.equal(doc.activeElement, source.children[0]);
});
test("board pointer capture pans the native surface and emits domain range", () => {
  dom();
  const board = new Element("board"); board.dataset = { pointer: "board", start: "0", end: "100" }; board.scrollLeft = 100; board.scrollTop = 100;
  pointers(board);
  board.listeners.get("pointerdown")({ target: board, pointerId: 3, clientX: 100, clientY: 20 });
  board.listeners.get("pointermove")({ pointerId: 3, clientX: 140, clientY: 40 });
  board.listeners.get("pointerup")({ pointerId: 3 });
  assert.equal(board.capture, 3); assert.equal(board.scrollLeft, 60); assert.equal(board.scrollTop, 80);
  assert.deepEqual(board.events[0].detail, { kind: "board", start: -10, end: 90, deltaY: 20, phase: "preview" });
  assert.equal(board.events[1].detail.phase, "commit"); assert.equal(board.dataset.start, "-10");
});

test("generated key declaration drives browser dispatch including sequences and modifiers", () => {
  assert.equal(resolveKey("j", false), "next"); assert.equal(resolveKey("k", false), "previous");
  assert.equal(resolveKey("k", true), "palette"); assert.equal(resolveKey("g", false, "g"), "first");
  assert.equal(resolveKey("c", false, "y"), "copy_command"); assert.equal(resolveKey("Y", false), "copy_default");
  assert.equal(resolveKey("y", false), "copy_menu"); assert.equal(resolveKey("z", false), null);
});


test("oversized windows trim contiguous edges and keep every next row reachable", () => {
  const rows = Array.from({ length: 1000 }, (_, key) => ({ key, visible: key >= 10 && key < 20, protected: false }));
  const plan = core.windowTrim(rows, 50, "before");
  const removed = new Set(plan.remove);
  const retained = rows.filter(row => !removed.has(row.key)).map(row => row.key);
  assert.deepEqual(retained, Array.from({ length: 50 }, (_, index) => index + 10));
  assert.equal(plan.before, true); assert.equal(plan.after, true);
  // Directional links can fetch before first and after last; there is no middle hole.
  for (let index = 1; index < retained.length; index++) assert.equal(retained[index], retained[index - 1] + 1);
});
test("a protected window boundary stops pruning and preserves the interval to visible rows", () => {
  const rows = Array.from({ length: 1000 }, (_, key) => ({ key, visible: key >= 10 && key < 20, protected: key === 100 }));
  const plan = core.windowTrim(rows, 50, "before");
  const removed = new Set(plan.remove);
  const retained = rows.filter(row => !removed.has(row.key)).map(row => row.key);
  assert.deepEqual(retained, Array.from({ length: 91 }, (_, index) => index + 10));
  assert.ok(retained.includes(100));
  assert.ok(retained.length > 50, "preserving focus/selection takes precedence over nominal capacity");
});
