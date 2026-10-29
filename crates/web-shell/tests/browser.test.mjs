import { readFileSync } from "node:fs";
import test from "node:test";
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
const core = await import(pathToFileURL(process.env.CORE_MODULE));
const { FragmentController, copyTarget, pointers, resolveKey, bindFeed, protectedTarget, pruneWindow, keydown, presentSnapshot } = await import(pathToFileURL(process.env.SHELL_MODULE));

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
  closest(selector) {
    if (selector === "*" || (selector === "[data-row]" && "row" in this.dataset) || (selector === "[data-follow]" && "follow" in this.dataset)) return this;
    return this.parent?.closest(selector) ?? null;
  }
  contains(element) { return element?.parent === this; }
  querySelector(selector) {
    if (selector.includes("aria-selected")) return this.selected ?? null;
    if (selector === "a[data-open], a") return this.primary ?? null;
    return this.querySelectorAll(selector)[0] ?? null;
  }
  querySelectorAll(selector) {
    if (selector === "[data-row]") return this.children.filter(child => "row" in child.dataset);
    if (selector === "[data-window-before]") return this.children.filter(child => "windowBefore" in child.dataset);
    if (selector === "[data-window-after]") return this.children.filter(child => "windowAfter" in child.dataset);
    return [];
  }
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  dispatchEvent(event) { this.events.push(event); return true; }
  setPointerCapture(id) { this.capture = id; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 400, bottom: 300 }; }
  setAttribute(name, value) { this.attributes.set(name, value); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  hasAttribute(name) { return this.attributes.has(name); }
  click() { this.clicks = (this.clicks ?? 0) + 1; this.listeners.get("click")?.(); }
  scrollIntoView() {}
  focus() { document.activeElement = this; }
  select() { this.textSelected = true; }
  after(element) { document.notices.push(element); }
  replaceChildren(fragment) { this.html = fragment.html; this.scrollHeight += 25; }
  append(fragment) { if (fragment instanceof Element) this.children.push(fragment); else { this.html += fragment.html; this.scrollHeight += 25; } }
  prepend(child) { this.children.unshift(child); child.parent = this; }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(child => child !== this); document.notices = document.notices.filter(element => element !== this); }
}
function dom() {
  const reader = new Element("reader"); reader.dataset.follow = "false";
  const doc = {
    notices: [], rows: [], body: new Element("body"), activeElement: null,
    getElementById(id) { return id === "reader" ? reader : null; },
    getSelection() { return { toString: () => "", rangeCount: 0 }; },
    querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; },
    querySelectorAll(selector) { return selector === "[data-updates]" ? this.notices : selector === "[data-row]" ? this.rows : []; },
    createElement(tag) {
      if (tag === "template") {
        const content = { html: "", querySelector: () => null, querySelectorAll: () => [] };
        return { content, set innerHTML(value) { content.html = value; } };
      }
      return new Element();
    }
  };
  globalThis.Element = Element; globalThis.HTMLElement = Element; globalThis.document = doc; globalThis.location = { href: "https://example.test/reader" };
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

class Feed {
  constructor() { this.listeners = new Map(); this.closed = false; }
  addEventListener(type, callback) { this.listeners.set(type, callback); }
  emit(type, data, lastEventId = "") { this.listeners.get(type)?.({ data, lastEventId }); }
  close() { this.closed = true; }
}
const turn = () => new Promise(resolve => setImmediate(resolve));
function liveReader() {
  const state = dom(); state.reader.dataset.follow = "true"; state.reader.scrollTop = 200; state.doc.activeElement = state.doc.body;
  const feed = new Feed(), positions = [], statuses = [];
  const bound = bindFeed(feed, update => state.controller.receive(update), position => positions.push(position), message => statuses.push(message));
  return { ...state, feed, positions, statuses, bound };
}
function fragment(feed, position, update) { feed.emit("fragment", JSON.stringify({ target: "reader", ...update }), position); }

test("shared Resume reload frame closes the stream and explains that the view needs reload", async () => {
  dom(); const feed = new Feed(), statuses = [], positions = [], received = [];
  const bound = bindFeed(feed, async value => { received.push(value); }, value => positions.push(value), value => statuses.push(value));
  const fixture = readFileSync(new URL("reload.sse", import.meta.url), "utf8");
  const event = fixture.split("\n").find(line => line.startsWith("event: ")).slice(7);
  const data = fixture.split("\n").filter(line => line.startsWith("data: ")).map(line => line.slice(6)).join("\n");
  feed.emit(event, data);
  assert.equal(feed.closed, true); assert.equal(bound.requiresReload(), true); assert.match(statuses.at(-1), /position no longer retained.*Reload the view/);
  assert.equal(feed.listeners.has("gap"), false);
  fragment(feed, "later", { kind: "append", html: "late" }); await bound.settled();
  assert.deepEqual(received, []); assert.deepEqual(positions, []);
});
test("a delayed ref retains following appends and the stream cursor waits for their order", async () => {
  const state = liveReader(); let reply, signal;
  globalThis.fetch = async (_url, options) => { signal = options.signal; return new Promise(resolve => { reply = resolve; }); };
  fragment(state.feed, "p1", { kind: "ref", url: "/fragment?at=p1" }); await turn();
  fragment(state.feed, "p2", { kind: "append", html: "B" });
  fragment(state.feed, "p3", { kind: "append", html: "C" }); await turn();
  assert.equal(signal.aborted, false); assert.equal(state.reader.html, "old"); assert.deepEqual(state.positions, []);
  reply({ ok: true, text: async () => "newA" }); await state.bound.settled();
  assert.equal(state.reader.html, "newABC"); assert.deepEqual(state.positions, ["p1", "p2", "p3"]); assert.equal(state.feed.closed, false);
});
test("a newer inline replacement supersedes a ref even if its cancelled transport replies late", async () => {
  const state = liveReader(); let reply, signal;
  globalThis.fetch = async (_url, options) => { signal = options.signal; return new Promise(resolve => { reply = resolve; }); };
  fragment(state.feed, "p1", { kind: "ref", url: "/fragment?at=p1" }); await turn();
  fragment(state.feed, "p2", { kind: "replace", html: "newC" });
  fragment(state.feed, "p3", { kind: "append", html: "B" });
  assert.equal(signal.aborted, true); assert.equal(state.reader.html, "newCB");
  reply({ ok: true, text: async () => "staleA" }); await state.bound.settled();
  assert.equal(state.reader.html, "newCB"); assert.deepEqual(state.positions, ["p1", "p2", "p3"]);
});
test("a newer ref supersedes an older snapshot and keeps its own following append", async () => {
  const state = liveReader(); let oldReply, oldSignal;
  globalThis.fetch = async (address, options) => {
    if (address.searchParams.get("at") === "p1") { oldSignal = options.signal; return new Promise(resolve => { oldReply = resolve; }); }
    return { ok: true, text: async () => "newC" };
  };
  fragment(state.feed, "p1", { kind: "ref", url: "/fragment?at=p1" }); await turn();
  fragment(state.feed, "p2", { kind: "ref", url: "/fragment?at=p2" });
  fragment(state.feed, "p3", { kind: "append", html: "B" }); await turn();
  assert.equal(oldSignal.aborted, true); assert.equal(state.reader.html, "newCB");
  oldReply({ ok: true, text: async () => "staleA" }); await state.bound.settled();
  assert.equal(state.reader.html, "newCB"); assert.deepEqual(state.positions, ["p1", "p2", "p3"]);
});
test("a failed reference stops the feed without acknowledging it or later buffered appends", async () => {
  const state = liveReader();
  globalThis.fetch = async () => ({ ok: false, status: 503 });
  fragment(state.feed, "p1", { kind: "ref", url: "/fragment?at=p1" });
  fragment(state.feed, "p2", { kind: "append", html: "B" }); await state.bound.settled();
  assert.equal(state.reader.html, "old"); assert.deepEqual(state.positions, []); assert.equal(state.feed.closed, true);
  assert.match(state.statuses.at(-1), /503.*Reload the view/);
});
test("window pruning keeps the selected row itself after focus leaves it", () => {
  const { doc } = dom(); doc.activeElement = doc.body;
  const list = new Element("list"); list.dataset.window = "1";
  const items = Array.from({ length: 4 }, (_, index) => {
    const row = new Element(`r${index}`); row.dataset.row = ""; row.parent = list;
    row.getBoundingClientRect = () => index === 2 ? { top: 10, bottom: 20 } : { top: -20, bottom: -10 };
    return row;
  });
  list.children = [...items]; items[0].setAttribute("aria-selected", "true");
  assert.equal(protectedTarget(items[0]), true);
  pruneWindow(list, "https://example.test/window", "before");
  assert.ok(list.children.includes(items[0])); assert.ok(list.children.includes(items[2]));
  assert.equal(items[0].getAttribute("aria-selected"), "true");
});
test("alternating touch moves use each pointer's own delta and pinch the actual distance", () => {
  dom(); const board = new Element("board"); board.dataset = { pointer: "board", start: "0", end: "100" }; board.scrollLeft = 100;
  pointers(board);
  const down = (pointerId, clientX) => board.listeners.get("pointerdown")({ target: board, pointerId, clientX, clientY: 20 });
  const move = (pointerId, clientX) => board.listeners.get("pointermove")({ pointerId, clientX, clientY: 20 });
  down(1, 100); down(2, 300); move(1, 90);
  assert.equal(board.scrollLeft, 110);
  const first = board.events[0].detail;
  assert.ok(Math.abs(first.start - 25 / 7) < 1e-10); assert.ok(Math.abs(first.end - 2075 / 21) < 1e-10);
  move(2, 310); assert.equal(board.scrollLeft, 100);
  const second = board.events[1].detail;
  assert.ok(Math.abs((second.end - second.start) - 100 * 200 / 220) < 1e-10);
  board.listeners.get("pointerup")({ pointerId: 1 }); move(2, 320);
  assert.equal(board.scrollLeft, 90);
  const final = board.events.at(-1).detail;
  assert.ok(Math.abs((final.end - final.start) - (second.end - second.start)) < 1e-10, "remaining finger pans without stale pinch distance");
});
test("Enter keeps native row button and secondary-link actions; only row focus opens the primary link", () => {
  const { doc, reader } = dom(); reader.dataset.row = ""; doc.rows = [reader];
  const primary = new Element("primary"); primary.parent = reader; reader.primary = primary;
  const button = new Element("copy-button"); button.dataset.copyMenu = ""; button.parent = reader;
  const secondary = new Element("secondary"); secondary.parent = reader;
  for (const control of [button, secondary]) {
    doc.activeElement = control;
    const event = { target: control, key: "Enter", ctrlKey: false, preventDefault() { this.defaultPrevented = true; } };
    keydown(event);
    assert.equal(Boolean(event.defaultPrevented), false); if (!event.defaultPrevented) control.click();
    assert.equal(control.clicks, 1); assert.equal(primary.clicks ?? 0, 0);
  }
  doc.activeElement = reader;
  const event = { target: reader, key: "Enter", ctrlKey: false, preventDefault() { this.defaultPrevented = true; } };
  keydown(event); assert.equal(event.defaultPrevented, true); assert.equal(primary.clicks, 1);
});


test("closing a hidden feed prevents late work from overwriting the next connection's cursor", async () => {
  const feed = new Feed(), positions = [], statuses = []; let finish;
  const bound = bindFeed(feed, () => new Promise(resolve => { finish = resolve; }), position => positions.push(position), message => statuses.push(message));
  fragment(feed, "old-connection", { kind: "append", html: "A" });
  bound.close(); finish(); await bound.settled();
  assert.equal(feed.closed, true); assert.equal(bound.requiresReload(), false); assert.deepEqual(positions, []); assert.deepEqual(statuses, []);
});


test("a rejected event with no error value still closes the feed without acknowledging it", async () => {
  const feed = new Feed(), positions = [], statuses = [];
  const bound = bindFeed(feed, () => Promise.reject(undefined), position => positions.push(position), message => statuses.push(message));
  fragment(feed, "failed", { kind: "append", html: "A" }); await bound.settled();
  assert.equal(feed.closed, true); assert.deepEqual(positions, []); assert.match(statuses.at(-1), /Reload the view/);
});


test("a superseded visibility snapshot cannot commit its position after its reference finishes", async () => {
  let generation = 1, hidden = false, finish;
  const positions = [];
  const first = presentSnapshot({ position: "old", fragments: [{ kind: "ref", target: "reader", url: "/fragment?at=old" }] },
    () => new Promise(resolve => { finish = resolve; }), () => generation === 1 && !hidden, position => positions.push(position));
  generation = 2;
  assert.equal(await presentSnapshot({ position: "new", fragments: [] }, async () => {}, () => generation === 2 && !hidden, position => positions.push(position)), true);
  finish(); assert.equal(await first, false); assert.deepEqual(positions, ["new"]);
  let finishHidden;
  const next = presentSnapshot({ position: "hidden", fragments: [{ kind: "ref", target: "reader", url: "/fragment?at=hidden" }] },
    () => new Promise(resolve => { finishHidden = resolve; }), () => generation === 2 && !hidden, position => positions.push(position));
  hidden = true; finishHidden(); assert.equal(await next, false); assert.deepEqual(positions, ["new"]);
});
