// typescript/core.ts
var LatestRequest = class {
  current = /* @__PURE__ */ new Map();
  generation = 0;
  async run(key, read, apply) {
    this.current.get(key)?.controller.abort();
    const request = { controller: new AbortController(), generation: ++this.generation };
    this.current.set(key, request);
    try {
      const value = await read(request.controller.signal);
      if (request.controller.signal.aborted || this.current.get(key) !== request) return false;
      apply(value);
      return true;
    } catch (error) {
      if (request.controller.signal.aborted) return false;
      throw error;
    } finally {
      if (this.current.get(key) === request) this.current.delete(key);
    }
  }
  cancel(key) {
    this.current.get(key)?.controller.abort();
    this.current.delete(key);
  }
  abortAll() {
    for (const request of this.current.values()) request.controller.abort();
    this.current.clear();
  }
};
function shouldHold(kind, state) {
  return state.protected || kind === "append" && (!state.following || !state.atEnd);
}
function matchScore(query, label) {
  const needle = Array.from(query.toLowerCase());
  const haystack = Array.from(label.toLowerCase());
  let next = 0, score = 0, previous = -2;
  for (const character of needle) {
    let found = -1;
    for (let index = next; index < haystack.length; index++) {
      if (haystack[index] === character) {
        found = index;
        break;
      }
    }
    if (found < 0) return null;
    score += found === previous + 1 ? 8 : 1;
    if (found === 0 || /[ \t\n\r\v\f_/-]/u.test(haystack[found - 1] ?? "")) score += 4;
    previous = found;
    next = found + 1;
  }
  return score - (haystack.length - needle.length) / 1e3;
}
function nearEnd(scrollTop, height, total) {
  return total - scrollTop - height <= 2;
}
function pointerRange(start, end, width, delta, scale, fraction) {
  const span = end - start;
  const nextSpan = span / scale;
  const anchor = start + fraction * span;
  const nextStart = anchor - fraction * nextSpan - delta / Math.max(1, width) * span;
  return [nextStart, nextStart + nextSpan];
}
function sameOrigin(value, base) {
  const url2 = new URL(value, base);
  if (url2.origin !== new URL(base).origin || !["http:", "https:"].includes(url2.protocol)) throw new Error("Operation URL must be on this origin");
  return url2;
}
async function copyText(text, clipboard) {
  try {
    if (!clipboard) return { copied: false, characters: Array.from(text).length };
    await clipboard.writeText(text);
    return { copied: true, characters: Array.from(text).length };
  } catch {
    return { copied: false, characters: Array.from(text).length };
  }
}
function windowTrim(rows2, maximum, preferred) {
  const remaining = [...rows2];
  const remove = [];
  const result = { remove, before: false, after: false };
  const edges = [preferred, preferred === "before" ? "after" : "before"];
  for (const edge of edges) {
    while (remaining.length > maximum) {
      const row = edge === "before" ? remaining[0] : remaining.at(-1);
      if (!row || row.visible || row.protected) break;
      remove.push(row.key);
      result[edge] = true;
      if (edge === "before") remaining.shift();
      else remaining.pop();
    }
  }
  return result;
}

// typescript/types.ts
var SHARED_KEYS = [{ "key": "/", "prefix": null, "ctrl": false, "action": "focus_query", "picker": null, "place": "everywhere", "label": "Focus query" }, { "key": ":", "prefix": null, "ctrl": false, "action": "palette", "picker": null, "place": "everywhere", "label": "Actions palette" }, { "key": "k", "prefix": null, "ctrl": true, "action": "palette", "picker": null, "place": "everywhere", "label": "Actions palette" }, { "key": "?", "prefix": null, "ctrl": false, "action": "help", "picker": null, "place": "everywhere", "label": "Key help" }, { "key": "g", "prefix": null, "ctrl": false, "action": "go_prefix", "picker": null, "place": "everywhere", "label": "Go to section (then a letter)" }, { "key": "n", "prefix": "g", "ctrl": false, "action": "go_notices", "picker": null, "place": "everywhere", "label": "Go to notices" }, { "key": "o", "prefix": "g", "ctrl": false, "action": "go_counterpart", "picker": null, "place": "everywhere", "label": "Go to this view in the other product" }, { "key": "j", "prefix": null, "ctrl": false, "action": "next", "picker": "Ctrl-j", "place": "everywhere", "label": "Next item" }, { "key": "ArrowDown", "prefix": null, "ctrl": false, "action": "next", "picker": "ArrowDown", "place": "everywhere", "label": "Next item" }, { "key": "k", "prefix": null, "ctrl": false, "action": "previous", "picker": "Ctrl-k", "place": "everywhere", "label": "Previous item" }, { "key": "ArrowUp", "prefix": null, "ctrl": false, "action": "previous", "picker": "ArrowUp", "place": "everywhere", "label": "Previous item" }, { "key": "g", "prefix": "g", "ctrl": false, "action": "first", "picker": null, "place": "everywhere", "label": "First item" }, { "key": "G", "prefix": null, "ctrl": false, "action": "last", "picker": "Ctrl-Shift-g", "place": "everywhere", "label": "Last item" }, { "key": "Enter", "prefix": null, "ctrl": false, "action": "open", "picker": "Enter", "place": "everywhere", "label": "Open focused item or enter its card" }, { "key": "x", "prefix": null, "ctrl": false, "action": "toggle_selection", "picker": "Ctrl-x", "place": "everywhere", "label": "Toggle selection" }, { "key": "*", "prefix": null, "ctrl": false, "action": "select_visible", "picker": null, "place": "everywhere", "label": "Select visible items" }, { "key": "y", "prefix": null, "ctrl": false, "action": "copy_menu", "picker": "Ctrl-y", "place": "everywhere", "label": "Copy menu" }, { "key": "Y", "prefix": null, "ctrl": false, "action": "copy_default", "picker": "Ctrl-Shift-y", "place": "everywhere", "label": "Copy default form" }, { "key": "c", "prefix": "y", "ctrl": false, "action": "copy_command", "picker": null, "place": "everywhere", "label": "Copy CLI command" }, { "key": "n", "prefix": null, "ctrl": false, "action": "next_match", "picker": "Ctrl-n", "place": "everywhere", "label": "Next find match" }, { "key": "N", "prefix": null, "ctrl": false, "action": "previous_match", "picker": "Ctrl-Shift-n", "place": "everywhere", "label": "Previous find match" }, { "key": "ArrowLeft", "prefix": null, "ctrl": false, "action": "pan_earlier", "picker": null, "place": "timeline", "label": "Pan earlier" }, { "key": "ArrowRight", "prefix": null, "ctrl": false, "action": "pan_later", "picker": null, "place": "timeline", "label": "Pan later" }, { "key": "-", "prefix": null, "ctrl": false, "action": "zoom_out", "picker": null, "place": "timeline", "label": "Zoom out" }, { "key": "=", "prefix": null, "ctrl": false, "action": "zoom_in", "picker": null, "place": "timeline", "label": "Zoom in" }, { "key": "Escape", "prefix": null, "ctrl": false, "action": "close", "picker": "Escape", "place": "everywhere", "label": "Close, leave a card and clear selection" }];
var RELOAD_EVENT = "reload";
var FRAGMENT_EVENT = "fragment";
var SUBSCRIPTIONS_PARAMETER = "subscriptions";

// typescript/feed.ts
var COALESCE_MILLISECONDS = 20;
var RETRY_LIMIT_MILLISECONDS = 3e4;
var FeedHub = class {
  constructor(open, timer) {
    this.open = open;
    this.timer = timer;
  }
  entries = /* @__PURE__ */ new Map();
  connections = /* @__PURE__ */ new Map();
  nextPort = 0;
  connect(port) {
    const number = this.nextPort++;
    port.onmessage = (event) => this.receive(port, number, event.data);
  }
  /** Open event streams; at most one per route. */
  openStreams() {
    return [...this.connections.values()].filter((connection) => connection.stream).length;
  }
  receive(port, number, message) {
    if (message.kind === "subscribe") {
      this.entries.set(`${number}.${message.id}`, { port, tabId: message.id, route: message.route, reference: message.reference, since: message.since });
      this.changed(message.route);
    } else if (message.kind === "unsubscribe") {
      const key = `${number}.${message.id}`;
      const entry = this.entries.get(key);
      if (entry) {
        this.entries.delete(key);
        this.changed(entry.route);
      }
    } else {
      for (const [key, entry] of [...this.entries]) {
        if (entry.port === port) {
          this.entries.delete(key);
          this.changed(entry.route);
        }
      }
    }
  }
  connection(route) {
    let connection = this.connections.get(route);
    if (!connection) {
      connection = { stream: null, pending: null, failures: 0 };
      this.connections.set(route, connection);
    }
    return connection;
  }
  changed(route, delay = COALESCE_MILLISECONDS) {
    const connection = this.connection(route);
    if (connection.pending) return;
    connection.pending = this.timer(() => {
      connection.pending = null;
      this.reopen(route);
    }, delay);
  }
  routeEntries(route) {
    return [...this.entries].filter(([, entry]) => entry.route === route);
  }
  /** Each subscription resumes from the last position delivered to its tab, so
   * a reconnection neither repeats nor skips an event. */
  reopen(route) {
    const connection = this.connection(route);
    connection.stream?.close();
    connection.stream = null;
    const entries = this.routeEntries(route);
    if (!entries.length) {
      this.connections.delete(route);
      return;
    }
    const subscriptions = entries.map(([id, entry]) => ({ id, reference: entry.reference, since: entry.since }));
    const address = `${route}${route.includes("?") ? "&" : "?"}${SUBSCRIPTIONS_PARAMETER}=${encodeURIComponent(JSON.stringify(subscriptions))}`;
    const stream = this.open(address);
    connection.stream = stream;
    const current = () => connection.stream === stream;
    stream.addEventListener(FRAGMENT_EVENT, (event) => {
      if (!current()) return;
      let value;
      try {
        value = JSON.parse(event.data);
      } catch {
        this.reload(route, null, "the stream sent an unreadable event");
        return;
      }
      const entry = this.entries.get(value.subscription);
      if (!entry || entry.route !== route) return;
      if (event.lastEventId) entry.since = event.lastEventId;
      entry.port.postMessage({ kind: "fragment", id: entry.tabId, position: event.lastEventId, data: JSON.stringify(value.update) });
    });
    stream.addEventListener(RELOAD_EVENT, (event) => {
      if (!current()) return;
      let value;
      try {
        value = JSON.parse(event.data);
      } catch {
        value = { reason: event.data };
      }
      this.reload(route, value.subscription ?? null, value.reason);
    });
    stream.onopen = () => {
      if (!current()) return;
      connection.failures = 0;
      this.broadcast(route, { kind: "connection", state: "open" });
    };
    stream.onerror = () => {
      if (!current()) return;
      stream.close();
      connection.stream = null;
      this.broadcast(route, { kind: "connection", state: "retrying" });
      connection.failures += 1;
      connection.pending?.();
      connection.pending = null;
      this.changed(route, Math.min(RETRY_LIMIT_MILLISECONDS, 1e3 * 2 ** (connection.failures - 1)));
    };
  }
  reload(route, subscription, reason) {
    for (const [key, entry] of this.routeEntries(route)) {
      if (subscription !== null && key !== subscription) continue;
      entry.port.postMessage({ kind: "reload", id: entry.tabId, reason });
      this.entries.delete(key);
    }
    this.changed(route);
  }
  broadcast(route, message) {
    const ports = new Set(this.routeEntries(route).map(([, entry]) => entry.port));
    for (const port of ports) port.postMessage(message);
  }
};
function browserTimer(callback, delay) {
  const handle = setTimeout(callback, delay);
  return () => clearTimeout(handle);
}

// typescript/timeline.ts
var ROW_HEIGHT = 22;
var SHAPE_INSET = 4;
function xOf(time, window2, width) {
  return (time - window2.start) / (window2.end - window2.start) * width;
}
function timeOf(x, window2, width) {
  return window2.start + x / Math.max(1, width) * (window2.end - window2.start);
}
function rowKey(lane, group) {
  return JSON.stringify([lane, group]);
}
function rowEnd(row) {
  return row.shape.render === "point" ? row.start : row.shape.end;
}
function layout(series, window2, width) {
  const rows2 = [];
  const index = /* @__PURE__ */ new Map();
  for (const lane of series.lanes) {
    const groups = lane.group_by === null ? [null] : [...new Set(series.rows.filter((row) => row.lane === lane.name).map((row) => row.group ?? ""))].sort();
    for (const group of groups) {
      index.set(rowKey(lane.name, group), rows2.length);
      rows2.push({ lane: lane.name, group, label: group === null ? lane.label : `${lane.label} \xB7 ${group}`, top: rows2.length * ROW_HEIGHT });
    }
  }
  const minimum = series.minimum_pixels;
  const shapes = [];
  series.rows.forEach((item, position) => {
    const row = index.get(rowKey(item.lane, item.group));
    if (row === void 0) return;
    let x0 = xOf(item.start, window2, width);
    let x1 = xOf(rowEnd(item), window2, width);
    if (x1 - x0 < minimum) {
      const centre = item.shape.render === "point" ? x0 : (x0 + x1) / 2;
      x0 = centre - minimum / 2;
      x1 = centre + minimum / 2;
    }
    if (x1 < 0 || x0 > width) return;
    const top = row * ROW_HEIGHT + SHAPE_INSET;
    shapes.push({ index: position, row, x0: Math.max(0, x0), x1: Math.min(width, x1), top, bottom: top + ROW_HEIGHT - 2 * SHAPE_INSET, series: item });
  });
  return { rows: rows2, shapes, height: rows2.length * ROW_HEIGHT };
}
function shapeAt(drawn, x, y) {
  for (let index = drawn.shapes.length - 1; index >= 0; index--) {
    const shape = drawn.shapes[index];
    if (shape && x >= shape.x0 && x <= shape.x1 && y >= shape.top && y <= shape.bottom) return shape;
  }
  return null;
}
function step(drawn, current, direction) {
  const ordered = [...drawn.shapes].sort((left, right) => left.row - right.row || left.x0 - right.x0 || left.index - right.index);
  if (!ordered.length) return null;
  const at = ordered.findIndex((shape) => shape.index === current);
  if (at < 0) return direction > 0 ? ordered[0] ?? null : ordered.at(-1) ?? null;
  return ordered[Math.max(0, Math.min(ordered.length - 1, at + direction))] ?? null;
}
function zoom(window2, factor, fraction) {
  const [start, end] = pointerRange(window2.start, window2.end, 1, 0, factor, fraction);
  return { start: Math.round(start), end: Math.max(Math.round(start) + 1, Math.round(end)) };
}
function pan(window2, fraction) {
  const shift = Math.round((window2.end - window2.start) * fraction);
  return { start: window2.start + shift, end: window2.end + shift };
}
function countWindow(row) {
  return row.shape.render === "count" ? { start: row.start, end: row.shape.end } : null;
}
var STEPS = [1e3, 5e3, 15e3, 6e4, 3e5, 9e5, 36e5, 108e5, 216e5, 432e5, 864e5, 6048e5, 2592e6, 7776e6, 31536e6];
function ticks(window2, width, timeZone, spacing = 60) {
  const span = window2.end - window2.start;
  const size = STEPS.find((candidate) => candidate / span * width >= spacing) ?? STEPS.at(-1) ?? span;
  const options = size >= 864e5 ? { timeZone, year: size >= 2592e6 ? "numeric" : void 0, month: "short", day: size >= 2592e6 ? void 0 : "numeric" } : { timeZone, hour: "2-digit", minute: "2-digit", second: size < 6e4 ? "2-digit" : void 0, hourCycle: "h23" };
  const format = new Intl.DateTimeFormat("en", options);
  const result = [];
  for (let time = Math.ceil(window2.start / size) * size; time < window2.end; time += size) {
    result.push({ x: xOf(time, window2, width), label: format.format(time) });
  }
  return result;
}

// typescript/shell.ts
var requests = new LatestRequest();
function status(message) {
  const line = document.getElementById("shell-status");
  if (line) line.textContent = message;
}
function report(error) {
  status(error instanceof Error ? error.message : String(error));
}
function url(value) {
  return sameOrigin(value, location.href);
}
function one(selector, root = document) {
  return root.querySelector(selector);
}
function all(selector, root = document) {
  return Array.from(root.querySelectorAll(selector));
}
function target(event) {
  return event.target instanceof Element ? event.target.closest("*") : null;
}
function dialog(id) {
  return document.getElementById(id);
}
function activeContent(fragment) {
  return Boolean(fragment.querySelector("script,style,[style],iframe,object,embed"));
}
var lastFocusedRow = null;
var heldRow = null;
function openDialog(id) {
  const element = dialog(id);
  if (!element || element.open) return;
  heldRow = focusedRow() ?? lastFocusedRow;
  element.addEventListener("close", () => {
    heldRow = null;
    requests.cancel("palette");
  }, { once: true });
  element.showModal();
  if (id === "palette") void refreshActions().catch(report);
}
async function refreshActions() {
  const route = heldRow?.dataset.actionsRoute ?? document.body.dataset.actionsRoute;
  if (!route) return;
  const references = all('[data-row][aria-selected="true"]').map((row) => row.dataset.reference ?? row.id);
  if (!references.length && heldRow) references.push(heldRow.dataset.reference ?? heldRow.id);
  const address = url(route);
  address.searchParams.set("references", JSON.stringify(references));
  await requests.run("palette", async (signal) => {
    const response = await fetch(address, { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
    if (!response.ok) throw new Error(`Actions unavailable (${response.status})`);
    return response.text();
  }, (html) => {
    const container = one("[data-palette-actions]");
    const template = document.createElement("template");
    template.innerHTML = html;
    if (activeContent(template.content)) throw new Error("Actions contain unsupported active content");
    container?.replaceChildren(template.content);
    paletteFilter();
  });
}
function selectedText() {
  return Boolean(document.getSelection()?.toString());
}
function protectedTarget(element) {
  const active = document.activeElement;
  return Boolean(heldRow && (element === heldRow || element.contains(heldRow))) || active !== document.body && active !== element && Boolean(active && element.contains(active)) || active === element || element.getAttribute("aria-selected") === "true" || Boolean(one('[aria-selected="true"]', element)) || Boolean(selectedText() && document.getSelection()?.containsNode(element, true));
}
function rows(root = document) {
  return all("[data-row]", root).filter((row) => !row.hidden);
}
function focusedRow() {
  return document.activeElement?.closest("[data-row]") ?? null;
}
function focusRow(index, candidates = rows()) {
  const row = candidates[Math.max(0, Math.min(index, candidates.length - 1))];
  if (!row) return;
  if (!row.hasAttribute("tabindex")) row.tabIndex = -1;
  row.focus({ preventScroll: true });
  row.scrollIntoView({ block: "nearest" });
}
function toggleRow(row) {
  if (row) row.setAttribute("aria-selected", row.getAttribute("aria-selected") === "true" ? "false" : "true");
}
function openRow() {
  const row = focusedRow();
  one("a[data-open], a", row ?? document)?.click();
}
var FragmentController = class {
  constructor(doc = document) {
    this.doc = doc;
  }
  pending = /* @__PURE__ */ new Map();
  revisions = /* @__PURE__ */ new Map();
  resolving = /* @__PURE__ */ new Map();
  receive(update) {
    const element = this.doc.getElementById(update.target);
    if (!(element instanceof HTMLElement)) return Promise.resolve();
    let revision = this.revisions.get(update.target) ?? 0;
    if (update.kind !== "append") {
      this.revisions.set(update.target, ++revision);
      requests.cancel(`fragment:${update.target}`);
      this.resolving.delete(update.target);
    }
    const preceding = this.resolving.get(update.target);
    if (update.kind === "ref" || preceding) {
      const currentRevision = revision;
      const current = () => (this.revisions.get(update.target) ?? 0) === currentRevision;
      const work = (preceding ?? Promise.resolve()).then(async () => {
        if (!current()) return;
        if (update.kind === "ref") {
          const applied = await requests.run(`fragment:${update.target}`, async (signal) => {
            const response = await fetch(url(update.url), { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
            if (!response.ok) throw new Error(`Fragment unavailable (${response.status})`);
            return response.text();
          }, (html) => {
            if (current()) this.rendered(element, { kind: "replace", target: update.target, html });
          });
          if (!applied && current()) throw new Error("Fragment request interrupted. Reload the view.");
        } else this.rendered(element, update);
      }).catch((error) => {
        if (current()) throw error;
      });
      const finished = work.finally(() => {
        if (this.resolving.get(update.target) === finished) this.resolving.delete(update.target);
      });
      this.resolving.set(update.target, finished);
      return finished;
    }
    this.rendered(element, update);
    return Promise.resolve();
  }
  rendered(element, update) {
    const reader = element.closest("[data-follow]") ?? element;
    const state = { following: reader.dataset.follow === "true", atEnd: nearEnd(reader.scrollTop, reader.clientHeight, reader.scrollHeight), protected: protectedTarget(element) };
    if (this.pending.has(update.target) || shouldHold(update.kind, state)) {
      const pending = this.pending.get(update.target) ?? [];
      if (update.kind === "replace") {
        const previous = pending.findIndex((item) => item.kind === "replace");
        if (previous >= 0) pending.splice(previous, 1);
      }
      pending.push(update);
      this.pending.set(update.target, pending);
      this.notice(element, pending.length, update.kind === "append");
    } else this.apply(element, update, state.following && state.atEnd);
  }
  notice(element, count, append) {
    let button = all("[data-updates]").find((button2) => button2.dataset.updates === element.id);
    if (!button) {
      button = this.doc.createElement("button");
      button.type = "button";
      button.dataset.updates = element.id;
      button.addEventListener("click", () => this.flush(element.id));
      element.after(button);
    }
    button.textContent = append ? `${count} new \u2193` : `${count} updates \xB7 apply`;
  }
  flush(id) {
    const element = this.doc.getElementById(id);
    if (!(element instanceof HTMLElement)) return;
    if (protectedTarget(element)) {
      status("Move focus out of the updated item and clear its selection to apply updates.");
      return;
    }
    for (const update of this.pending.get(id) ?? []) {
      if (update.kind !== "ref") this.apply(element, update, update.kind === "append");
    }
    this.pending.delete(id);
    all("[data-updates]").find((button) => button.dataset.updates === id)?.remove();
  }
  /** Inserted shell elements connect themselves: windows, boards, timelines, feeds and seen marks. */
  apply(element, update, follow2) {
    const reader = element.closest("[data-follow]") ?? element;
    const scroll = reader.scrollTop;
    const template = this.doc.createElement("template");
    template.innerHTML = update.html;
    if (activeContent(template.content)) {
      status("Fragment contains unsupported active content.");
      return;
    }
    if (update.kind === "append") element.append(template.content);
    else element.replaceChildren(template.content);
    reader.scrollTop = follow2 ? reader.scrollHeight : scroll;
  }
};
var fragments = typeof document === "undefined" ? null : new FragmentController();
function readUpdate(value) {
  if (typeof value !== "object" || value === null) throw new Error("Invalid fragment event");
  const item = value;
  if (typeof item.target !== "string" || !["append", "replace", "ref"].includes(String(item.kind))) throw new Error("Invalid fragment event");
  if (item.kind === "ref" ? typeof item.url !== "string" : typeof item.html !== "string") throw new Error("Invalid fragment payload");
  return item;
}
async function presentSnapshot(snapshot, receive, current, recordPosition) {
  if (!current()) return false;
  await Promise.all(snapshot.fragments.map((value) => receive(readUpdate(value))));
  if (!current()) return false;
  recordPosition(snapshot.position);
  return true;
}
function bindFeed(source, receive, recordPosition, showStatus = status) {
  let stopped = false;
  let reloadRequired = false;
  let ordered = Promise.resolve();
  const stop = (message) => {
    reloadRequired = true;
    stopped = true;
    source.close();
    showStatus(message);
  };
  source.addEventListener(FRAGMENT_EVENT, (event) => {
    if (stopped) return;
    try {
      const received = receive(readUpdate(JSON.parse(event.data)));
      const accounted = received.then(() => ({ ok: true }), (error) => ({ ok: false, error }));
      ordered = ordered.then(async () => {
        const result = await accounted;
        if (stopped) return;
        if (!result.ok) throw result.error;
        if (event.lastEventId) recordPosition(event.lastEventId);
      }).catch((error) => {
        if (!stopped) stop(`${error instanceof Error ? error.message : String(error)} Reload the view.`);
      });
    } catch (error) {
      stop(`${error instanceof Error ? error.message : String(error)} Reload the view.`);
    }
  });
  source.addEventListener(RELOAD_EVENT, (event) => {
    let reason = event.data;
    try {
      const value = JSON.parse(event.data);
      reason = value.reason ?? reason;
    } catch {
    }
    stop(`Subscription requires reload: ${reason}. Reload the view.`);
  });
  source.onopen = () => {
    if (!stopped) showStatus("");
  };
  source.onerror = () => {
    if (!stopped) showStatus("Disconnected \xB7 retrying");
  };
  return { settled: () => ordered, close: () => {
    stopped = true;
    source.close();
  }, requiresReload: () => reloadRequired };
}
var SubscriptionSource = class {
  constructor(leave) {
    this.leave = leave;
  }
  listeners = /* @__PURE__ */ new Map();
  onopen = null;
  onerror = null;
  addEventListener(type, listener) {
    this.listeners.set(type, listener);
  }
  emit(type, data, lastEventId = "") {
    this.listeners.get(type)?.({ data, lastEventId });
  }
  close() {
    this.leave();
  }
};
function deliver(message, sources2) {
  if (message.kind === "fragment") sources2.get(message.id)?.emit(FRAGMENT_EVENT, message.data, message.position);
  else if (message.kind === "reload") sources2.get(message.id)?.emit(RELOAD_EVENT, JSON.stringify({ reason: message.reason }));
  else for (const source of sources2.values()) (message.state === "open" ? source.onopen : source.onerror)?.();
}
var sources = /* @__PURE__ */ new Map();
var hubPort = null;
var nextSubscription = 0;
function hub() {
  if (hubPort) return hubPort;
  let port;
  try {
    port = new SharedWorker("/assets/feed-worker.js", { name: "web-shell feed" }).port;
  } catch {
    const local = new FeedHub((address) => new EventSource(address, { withCredentials: true }), browserTimer);
    const channel = new MessageChannel();
    local.connect(channel.port1);
    port = channel.port2;
  }
  port.onmessage = (event) => deliver(event.data, sources);
  hubPort = port;
  return port;
}
var following = /* @__PURE__ */ new WeakMap();
var feeds = /* @__PURE__ */ new Set();
var visibilityGeneration = 0;
async function follow(element, refresh, generation) {
  if (following.has(element) || element.closest("shell-preview")) return;
  const { route, reference, refresh: refreshRoute } = element.dataset;
  if (!route || !reference || !element.dataset.since) {
    status("A live feed has no route, reference or position. Reload the view.");
    return;
  }
  const current = () => generation === visibilityGeneration && !document.hidden && element.isConnected && !following.has(element);
  if (refresh && refreshRoute) {
    let snapshot;
    const applied = await requests.run(`refresh:${reference}`, async (signal) => {
      const response = await fetch(url(refreshRoute), { signal, credentials: "same-origin", headers: { Accept: "application/json" } });
      if (!response.ok) throw new Error(`Refresh unavailable (${response.status})`);
      return await response.json();
    }, (value) => {
      if (!Array.isArray(value.fragments) || typeof value.position !== "string") throw new Error("Refresh must contain fragments and their read position");
      snapshot = value;
    });
    if (!applied || !snapshot) return;
    const presented = await presentSnapshot(snapshot, (update) => fragments?.receive(update) ?? Promise.resolve(), current, (position) => {
      element.dataset.since = position;
    });
    if (!presented) return;
  }
  if (!current()) return;
  const id = `s${nextSubscription++}`;
  const source = new SubscriptionSource(() => {
    if (sources.delete(id)) hub().postMessage({ kind: "unsubscribe", id });
  });
  sources.set(id, source);
  const bound = bindFeed(source, (update) => fragments?.receive(update) ?? Promise.resolve(), (position) => {
    element.dataset.since = position;
  });
  following.set(element, { bound });
  hub().postMessage({ kind: "subscribe", route: url(route).href, id, reference, since: element.dataset.since });
}
function unfollow(element, removed) {
  const state = following.get(element);
  if (!state || state.bound.requiresReload() && !removed) return;
  state.bound.close();
  following.delete(element);
}
async function visibility(returning) {
  const generation = ++visibilityGeneration;
  if (document.hidden) {
    for (const element of feeds) unfollow(element, false);
    requests.abortAll();
    return;
  }
  await Promise.all([...feeds].map((element) => follow(element, returning, generation).catch(report)));
}
var copySource = null;
async function copyTarget(source, formIndex) {
  const value = JSON.parse(source.dataset.copy ?? "null");
  const form = value?.forms[formIndex];
  if (!value || !form) return;
  const result = await copyText(form.text, navigator.clipboard);
  const feedback = one("[data-copy-feedback]", source) ?? source;
  source.querySelector(".copy-fallback")?.remove();
  if (result.copied) feedback.textContent = `${value.label} as ${form.label} copied \xB7 ${result.characters.toLocaleString()} characters`;
  else {
    feedback.textContent = "Clipboard unavailable. Select and copy the text below.";
    const box = document.createElement("textarea");
    box.className = "copy-fallback";
    box.readOnly = true;
    box.value = form.text;
    box.setAttribute("aria-label", `${value.label} as ${form.label}`);
    source.append(box);
    box.focus();
    box.select();
  }
}
function copyMenu(source) {
  copySource = source;
  const value = JSON.parse(source.dataset.copy ?? "null");
  const forms = one("[data-copy-forms]");
  if (!forms || !value) return;
  forms.replaceChildren();
  value.forms.forEach((form, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = form.label;
    button.addEventListener("click", () => {
      dialog("copy-menu")?.close();
      if (copySource) void copyTarget(copySource, index).catch(report);
    });
    forms.append(button);
  });
  openDialog("copy-menu");
}
function currentCopy() {
  return focusedRow()?.closest("[data-copy]") ?? one("[data-copy]", focusedRow() ?? document);
}
function copyCommand() {
  dialog("copy-menu")?.close();
  const source = document.getElementById("page-command")?.closest("[data-copy]");
  if (source) void copyTarget(source, 0).catch(report);
}
function paletteFilter() {
  const query = one("#palette-query")?.value ?? "";
  for (const item of all("[data-action], [data-palette-navigation]", dialog("palette") ?? document)) {
    const action = item.dataset.action ? JSON.parse(item.dataset.action) : null;
    const score = matchScore(query, `${item.textContent ?? ""} ${action?.cli ?? ""}`);
    const wrapper = item.closest("[data-action-item]") ?? item;
    wrapper.hidden = score === null;
  }
}
async function submit(action, signal) {
  const response = await fetch(url(action.route), { method: "POST", signal, credentials: "same-origin", headers: { "Content-Type": "application/json", Accept: "text/html" }, body: action.request });
  if (!response.ok) throw new Error(`${action.label} failed (${response.status}). Editor text is retained.`);
  return await response.text();
}
async function runAction(action) {
  const reason = "reason" in action.availability ? action.availability.reason : action.availability.state;
  if (!["enabled", "partial"].includes(action.availability.state)) {
    status(`${action.label}: ${reason}`);
    return;
  }
  if (!action.write) {
    const address = url(action.route);
    const request = JSON.parse(action.request);
    for (const [key, value] of Object.entries(request.arguments)) {
      if (value !== null && value !== void 0) address.searchParams.set(key, typeof value === "string" ? value : JSON.stringify(value));
    }
    location.assign(address.href);
    return;
  }
  await requests.run(`operation:${action.operation}`, (signal) => submit(action, signal), (html) => {
    status(`${action.label} completed`);
    if (html.trimStart().startsWith("<")) {
      const selected = heldRow ?? focusedRow();
      if (selected?.id) void fragments?.receive({ kind: "replace", target: selected.id, html }).catch(report);
    }
    dialog("palette")?.close();
  });
}
function displayed(state) {
  return state.visibilityState === "visible" && !state.prerendering && state.intersecting && !state.inPreview && state.rendered;
}
var SeenMark = class {
  constructor(send, failed) {
    this.send = send;
    this.failed = failed;
  }
  sent = false;
  sending = false;
  async update(state) {
    if (this.sent || this.sending || !displayed(state)) return false;
    this.sending = true;
    try {
      await this.send();
      this.sent = true;
      return true;
    } catch (error) {
      this.failed(error);
      return false;
    } finally {
      this.sending = false;
    }
  }
};
function enterCard(card) {
  card.setAttribute("data-entered", "");
  card.focus();
  const keys = all("[data-card-key]", card).map((button) => button.dataset.cardKey).join(" ");
  status(`Card keys: ${keys} \xB7 Esc leaves the card`);
}
function leaveCard(card) {
  card.removeAttribute("data-entered");
  (card.closest("[data-row]") ?? card).focus();
  status("");
}
function timelineOf(element) {
  const timeline = element?.closest("shell-timeline");
  return timeline && "stepShape" in timeline ? timeline : null;
}
var prefix = "";
var prefixExpires = 0;
function resolveKey(key, ctrl, prefix2 = "") {
  return SHARED_KEYS.find((binding) => binding.key === key && binding.ctrl === ctrl && (binding.prefix ?? "") === prefix2)?.action ?? null;
}
function keydown(event) {
  const element = target(event);
  if (event.isComposing || event.altKey || event.metaKey) return;
  const card = element?.closest("shell-card") ?? null;
  if (card?.hasAttribute("data-entered")) {
    if (event.key === "Escape") {
      event.preventDefault();
      leaveCard(card);
      return;
    }
    if (!event.ctrlKey && event.key.length === 1) {
      const button = all("[data-card-key]", card).find((button2) => button2.dataset.cardKey === event.key);
      if (button) {
        event.preventDefault();
        if (!button.disabled) button.click();
        return;
      }
    }
  }
  if (resolveKey(event.key, event.ctrlKey) === "close") {
    const activeDialog = element?.closest("dialog");
    activeDialog?.close();
    const disclosure = element?.closest("details");
    if (disclosure) {
      disclosure.open = false;
      one("summary", disclosure)?.focus();
    }
    for (const row of all('[aria-selected="true"]')) row.setAttribute("aria-selected", "false");
    element?.closest("shell-editor")?.setAttribute("hidden", "");
    prefix = "";
    return;
  }
  const directAction = resolveKey(event.key, event.ctrlKey);
  if (element?.id === "palette-query" && (directAction === "open" || directAction === "next")) {
    const item = all("[data-action], [data-palette-navigation]", dialog("palette") ?? document).find((item2) => !item2.closest("[hidden]") && !item2.hasAttribute("disabled"));
    if (item) {
      event.preventDefault();
      if (directAction === "open") item.click();
      else item.focus();
    }
    return;
  }
  if (element?.closest("input,textarea,select,[contenteditable]:not([contenteditable=false])") || selectedText()) return;
  if (event.ctrlKey && directAction === "palette") {
    event.preventDefault();
    openDialog("palette");
    return;
  }
  if (event.ctrlKey) return;
  if (one("[data-shortcuts]")?.checked === false && event.key.length === 1) return;
  if (Date.now() > prefixExpires) prefix = "";
  const wasPrefix = prefix;
  prefix = "";
  const sequence = wasPrefix ? resolveKey(event.key, event.ctrlKey, wasPrefix) : null;
  const keyAction = sequence ?? directAction;
  if (resolveKey(wasPrefix, false) === "go_prefix" && sequence === null) {
    const link = all("[data-go]").find((link2) => link2.dataset.go === event.key);
    if (link) {
      event.preventDefault();
      link.click();
      return;
    }
  }
  const timeline = timelineOf(element);
  const palette = element?.closest("#palette");
  const candidates = palette ? all("[data-action], [data-palette-navigation]", palette).filter((item) => !item.closest("[hidden]") && !item.hasAttribute("disabled")) : rows();
  const index = candidates.indexOf(palette ? document.activeElement : focusedRow());
  switch (keyAction) {
    case "focus_query":
      event.preventDefault();
      one("[data-query]")?.focus();
      break;
    case "palette":
      event.preventDefault();
      openDialog("palette");
      break;
    case "help": {
      event.preventDefault();
      const help = one("#key-help");
      if (help) help.open = !help.open;
      break;
    }
    case "go_prefix":
      prefix = event.key;
      prefixExpires = Date.now() + 1e3;
      break;
    case "go_notices": {
      event.preventDefault();
      const notices = one("[data-notices]");
      if (notices) notices.click();
      else status("This view has no notices list.");
      break;
    }
    case "go_counterpart": {
      event.preventDefault();
      const counterpart = one("[data-counterpart]");
      if (counterpart?.getAttribute("href")) counterpart.click();
      else status(counterpart?.dataset.reason ?? "No other product is configured.");
      break;
    }
    case "next":
    case "previous": {
      event.preventDefault();
      const direction = keyAction === "next" ? 1 : -1;
      if (timeline) timeline.stepShape(direction);
      else focusRow(direction > 0 ? index + 1 : index < 0 ? candidates.length - 1 : index - 1, candidates);
      break;
    }
    case "first":
      event.preventDefault();
      if (timeline) timeline.stepShape(-1, true);
      else focusRow(0, candidates);
      break;
    case "last":
      event.preventDefault();
      if (timeline) timeline.stepShape(1, true);
      else focusRow(candidates.length - 1, candidates);
      break;
    case "pan_earlier":
    case "pan_later":
      if (timeline) {
        event.preventDefault();
        timeline.panBy(keyAction === "pan_later" ? 0.25 : -0.25);
      }
      break;
    case "zoom_in":
    case "zoom_out":
      if (timeline) {
        event.preventDefault();
        timeline.zoomBy(keyAction === "zoom_in" ? 2 : 0.5);
      }
      break;
    case "copy_command":
      event.preventDefault();
      copyCommand();
      break;
    case "open": {
      if (timeline) {
        event.preventDefault();
        timeline.openShape();
        break;
      }
      const row = focusedRow();
      const closedCard = card && !card.hasAttribute("data-entered") && element === card ? card : row && element === row ? one("shell-card", row) : null;
      if (closedCard) {
        event.preventDefault();
        enterCard(closedCard);
      } else if (row && element === row) {
        event.preventDefault();
        openRow();
      }
      break;
    }
    case "toggle_selection":
      event.preventDefault();
      toggleRow(focusedRow());
      break;
    case "select_visible":
      event.preventDefault();
      candidates.filter((row) => {
        const rect = row.getBoundingClientRect();
        return rect.bottom > 0 && rect.top < innerHeight;
      }).forEach((row) => row.setAttribute("aria-selected", "true"));
      break;
    case "copy_menu": {
      event.preventDefault();
      prefix = event.key;
      prefixExpires = Date.now() + 1e3;
      const source = currentCopy();
      if (source) copyMenu(source);
      break;
    }
    case "copy_default": {
      event.preventDefault();
      const source = currentCopy();
      if (source) void copyTarget(source, 0).catch(report);
      break;
    }
    case "next_match":
    case "previous_match": {
      event.preventDefault();
      const matches = all("[data-find-match]");
      const index2 = matches.findIndex((item) => item === document.activeElement);
      focusRow((index2 + (keyAction === "next_match" ? 1 : -1) + matches.length) % matches.length, matches);
      break;
    }
    default: {
      const action = all("[data-action]").find((button) => {
        const action2 = JSON.parse(button.dataset.action ?? "null");
        return action2?.key === event.key;
      });
      action?.click();
    }
  }
}
function enhanceWindow(list) {
  let previousScroll = list.scrollTop;
  list.addEventListener("scroll", () => {
    const direction = list.scrollTop >= previousScroll ? "after" : "before";
    previousScroll = list.scrollTop;
    const route = list.dataset.windowRoute ?? one("[data-window-before], [data-window-after]", list)?.href;
    if (route) pruneWindow(list, route, direction === "after" ? "before" : "after");
    if (list.scrollTop < list.clientHeight / 2) void windowPage(list, "before").catch(report);
    else if (list.scrollHeight - list.scrollTop - list.clientHeight < list.clientHeight / 2) void windowPage(list, "after").catch(report);
  }, { passive: true });
}
var paging = /* @__PURE__ */ new WeakSet();
async function windowPage(list, direction) {
  if (paging.has(list)) return;
  const link = one(`[data-window-${direction}]`, list);
  if (!link) return;
  paging.add(list);
  try {
    const existing = rows(list);
    const anchor = direction === "before" ? existing[0] : existing.at(-1);
    if (!anchor?.dataset.anchor) return;
    const address = url(link.href);
    address.searchParams.set("anchor", anchor.dataset.anchor);
    address.searchParams.set("direction", direction);
    await requests.run(`window:${list.id}`, async (signal) => {
      const response = await fetch(address, { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
      if (!response.ok) throw new Error(`Window unavailable (${response.status})`);
      return await response.text();
    }, (html) => {
      const top = anchor.getBoundingClientRect().top;
      const template = document.createElement("template");
      template.innerHTML = html;
      if (activeContent(template.content)) throw new Error("Window contains unsupported active content");
      const incoming = rows(template.content);
      const known = new Set(rows(list).map((row) => row.dataset.anchor));
      for (const row of incoming) if (known.has(row.dataset.anchor)) row.remove();
      if (direction === "before") list.prepend(template.content);
      else list.append(template.content);
      link.remove();
      list.scrollTop += anchor.getBoundingClientRect().top - top;
      list.dataset.windowRoute = link.href;
      pruneWindow(list, link.href, direction === "after" ? "before" : "after");
    });
  } finally {
    paging.delete(list);
  }
}
function pruneWindow(list, route, preferred) {
  const box = list.getBoundingClientRect();
  const candidates = rows(list).map((row) => {
    const rect = row.getBoundingClientRect();
    return { key: row, visible: rect.bottom > box.top && rect.top < box.bottom, protected: protectedTarget(row) };
  });
  const anchor = candidates.find((row) => row.visible)?.key;
  const top = anchor?.getBoundingClientRect().top;
  const maximum = Math.max(1, Number(list.dataset.size) || 200);
  const plan = windowTrim(candidates, maximum, preferred);
  plan.remove.forEach((row) => row.remove());
  for (const edge of ["before", "after"]) {
    if (!plan[edge] || one(`[data-window-${edge}]`, list)) continue;
    const link = document.createElement("a");
    link.href = route;
    link.dataset[edge === "before" ? "windowBefore" : "windowAfter"] = "";
    link.dataset.windowSentinel = "";
    link.textContent = `Load ${edge}`;
    if (edge === "before") list.prepend(link);
    else list.append(link);
  }
  if (anchor && top !== void 0) list.scrollTop += anchor.getBoundingClientRect().top - top;
}
function boardPointers(element) {
  const positions = /* @__PURE__ */ new Map();
  let start = 0, end = 1;
  const distance = () => {
    const [first, second] = [...positions.values()];
    return first && second ? Math.hypot(first.x - second.x, first.y - second.y) : void 0;
  };
  const range = () => {
    start = Number(element.dataset.start ?? 0);
    end = Number(element.dataset.end ?? 1);
  };
  const publish = (phase, deltaY = 0) => {
    element.dispatchEvent(new CustomEvent("shell:pointer", { bubbles: true, detail: { kind: "board", start, end, deltaY, phase } }));
    if (phase === "commit" && element.dataset.route && element.id) {
      const address = url(element.dataset.route);
      address.searchParams.set("start", String(start));
      address.searchParams.set("end", String(end));
      void fragments?.receive({ kind: "ref", target: element.id, url: address.href }).catch(report);
    }
  };
  element.addEventListener("pointerdown", (event) => {
    if (event.target.closest("a,button,input,textarea,select,[contenteditable], [data-row]")) return;
    if (positions.size === 0) range();
    positions.set(event.pointerId, { x: event.clientX, y: event.clientY });
    element.setPointerCapture(event.pointerId);
  });
  element.addEventListener("pointermove", (event) => {
    const previous = positions.get(event.pointerId);
    if (!previous) return;
    const previousDistance = distance();
    positions.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const nextDistance = distance();
    const scale = nextDistance && previousDistance ? nextDistance / previousDistance : 1;
    const deltaX = event.clientX - previous.x;
    const deltaY = event.clientY - previous.y;
    const box = element.getBoundingClientRect();
    [start, end] = pointerRange(start, end, box.width, deltaX, scale, (event.clientX - box.left) / Math.max(1, box.width));
    element.scrollLeft -= deltaX;
    element.scrollTop -= deltaY;
    publish("preview", deltaY);
  });
  const finish = (event) => {
    if (!positions.has(event.pointerId)) return;
    positions.delete(event.pointerId);
    element.dataset.start = String(start);
    element.dataset.end = String(end);
    publish("commit");
  };
  element.addEventListener("pointerup", finish);
  element.addEventListener("pointercancel", finish);
  element.addEventListener("wheel", (event) => {
    if (!event.ctrlKey) return;
    event.preventDefault();
    range();
    const box = element.getBoundingClientRect();
    [start, end] = pointerRange(start, end, box.width, 0, Math.exp(Math.max(-4, Math.min(4, -event.deltaY / 500))), (event.clientX - box.left) / Math.max(1, box.width));
    element.dataset.start = String(start);
    element.dataset.end = String(end);
    publish("commit");
  }, { passive: false });
}
var LABEL_WIDTH = 150;
var AXIS_HEIGHT = 20;
function drawTimeline(canvas, drawn, window2, timeZone, focused, selection, colours) {
  const context = canvas.getContext("2d");
  if (!context) return;
  const scale = devicePixelRatio || 1;
  const width = canvas.clientWidth;
  const height = AXIS_HEIGHT + drawn.height;
  canvas.width = Math.round(width * scale);
  canvas.height = Math.round(height * scale);
  canvas.style.height = `${height}px`;
  context.setTransform(scale, 0, 0, scale, 0, 0);
  context.clearRect(0, 0, width, height);
  context.font = "12px " + (colours.font ?? "sans-serif");
  context.textBaseline = "middle";
  const plot = width - LABEL_WIDTH;
  context.fillStyle = colours.muted ?? "#666";
  for (const tick of ticks(window2, plot, timeZone)) {
    context.fillText(tick.label, LABEL_WIDTH + tick.x + 2, AXIS_HEIGHT / 2);
    context.fillRect(LABEL_WIDTH + tick.x, AXIS_HEIGHT - 4, 1, drawn.height + 4);
  }
  for (const row of drawn.rows) {
    context.fillStyle = colours.ink ?? "#000";
    context.fillText(row.label, 4, AXIS_HEIGHT + row.top + ROW_HEIGHT / 2, LABEL_WIDTH - 8);
    context.fillStyle = colours.line ?? "#ccc";
    context.fillRect(0, AXIS_HEIGHT + row.top + ROW_HEIGHT - 1, width, 1);
  }
  context.save();
  context.beginPath();
  context.rect(LABEL_WIDTH, AXIS_HEIGHT, plot, drawn.height);
  context.clip();
  const largest = Math.max(1, ...drawn.shapes.map((shape) => shape.series.shape.render === "count" ? shape.series.shape.count : 1));
  const ink = colours.ink ?? "#000";
  for (const shape of drawn.shapes) drawShape(context, shape, ink, colours.accent ?? ink, largest);
  const focus = drawn.shapes.find((shape) => shape.index === focused);
  if (focus) {
    context.strokeStyle = colours.accent ?? ink;
    context.lineWidth = 2;
    context.strokeRect(LABEL_WIDTH + focus.x0 - 2, AXIS_HEIGHT + focus.top - 2, focus.x1 - focus.x0 + 4, focus.bottom - focus.top + 4);
  }
  if (selection) {
    const left = (selection.start - window2.start) / (window2.end - window2.start) * plot;
    const right = (selection.end - window2.start) / (window2.end - window2.start) * plot;
    context.globalAlpha = 0.18;
    context.fillStyle = colours.accent ?? ink;
    context.fillRect(LABEL_WIDTH + Math.min(left, right), AXIS_HEIGHT, Math.abs(right - left), drawn.height);
    context.globalAlpha = 1;
  }
  context.restore();
}
function drawShape(context, shape, ink, accent, largest) {
  const x = LABEL_WIDTH + shape.x0, y = AXIS_HEIGHT + shape.top, width = Math.max(1, shape.x1 - shape.x0), height = shape.bottom - shape.top;
  const row = shape.series;
  if (row.shape.render === "count") {
    context.globalAlpha = 0.25 + 0.75 * Math.log1p(row.shape.count) / Math.log1p(largest);
    context.fillStyle = ink;
    context.fillRect(x, y, width, height);
    context.globalAlpha = 1;
    const label2 = String(row.shape.count);
    if (context.measureText(label2).width + 4 < width) {
      context.fillStyle = accent;
      context.fillText(label2, x + 2, y + height / 2);
    }
    return;
  }
  const agency = row.shape.agency ?? "unknown";
  context.fillStyle = ink;
  context.strokeStyle = ink;
  context.lineWidth = 1;
  if (row.shape.render === "point") {
    const centre = x + width / 2;
    context.beginPath();
    context.moveTo(centre, y);
    context.lineTo(centre + height / 3, y + height / 2);
    context.lineTo(centre, y + height);
    context.lineTo(centre - height / 3, y + height / 2);
    context.closePath();
    if (agency === "agent") context.stroke();
    else context.fill();
    return;
  }
  if (agency === "operator") context.fillRect(x, y, width, height);
  else if (agency === "agent") context.strokeRect(x + 0.5, y + 0.5, width - 1, height - 1);
  else {
    context.strokeRect(x + 0.5, y + 0.5, width - 1, height - 1);
    context.save();
    context.beginPath();
    context.rect(x, y, width, height);
    context.clip();
    for (let offset = -height; offset < width; offset += 5) {
      context.beginPath();
      context.moveTo(x + offset, y + height);
      context.lineTo(x + offset + height, y);
      context.stroke();
    }
    context.restore();
  }
  const label = row.shape.label;
  if (label && context.measureText(label).width + 6 < width) {
    context.fillStyle = agency === "operator" ? accent : ink;
    context.fillText(label, x + 3, y + height / 2);
  }
}
function timelineElement() {
  return class ShellTimeline extends HTMLElement {
    canvas = null;
    description = null;
    series = null;
    drawn = null;
    focused = null;
    selection = null;
    dragFrom = null;
    touches = /* @__PURE__ */ new Map();
    fetchLater;
    resize = null;
    get window() {
      return { start: Number(this.dataset.start), end: Number(this.dataset.end) };
    }
    connectedCallback() {
      if (this.canvas) return;
      const canvas = document.createElement("canvas");
      canvas.setAttribute("aria-hidden", "true");
      const description = document.createElement("p");
      description.className = "timeline-focus";
      description.setAttribute("role", "status");
      description.setAttribute("aria-live", "polite");
      this.prepend(canvas);
      canvas.after(description);
      this.canvas = canvas;
      this.description = description;
      canvas.addEventListener("pointerdown", (event) => this.pointerDown(event));
      canvas.addEventListener("pointermove", (event) => this.pointerMove(event));
      canvas.addEventListener("pointerup", (event) => this.pointerUp(event));
      canvas.addEventListener("pointercancel", (event) => {
        this.touches.delete(event.pointerId);
        this.dragFrom = null;
      });
      canvas.addEventListener("dblclick", () => this.openShape());
      canvas.addEventListener("wheel", (event) => this.wheel(event), { passive: false });
      this.resize = new ResizeObserver(() => this.load());
      this.resize.observe(canvas);
    }
    disconnectedCallback() {
      this.resize?.disconnect();
      this.resize = null;
      this.canvas?.remove();
      this.description?.remove();
      this.canvas = null;
      requests.cancel(`timeline:${this.id}`);
    }
    plotWidth() {
      return Math.max(1, (this.canvas?.clientWidth ?? 0) - LABEL_WIDTH);
    }
    load() {
      const route = this.dataset.series;
      if (!route || !this.canvas) return;
      const address = url(route), window2 = this.window;
      address.searchParams.set("start", String(window2.start));
      address.searchParams.set("end", String(window2.end));
      address.searchParams.set("width", String(Math.round(this.plotWidth())));
      void requests.run(`timeline:${this.id}`, async (signal) => {
        const response = await fetch(address, { signal, credentials: "same-origin", headers: { Accept: "application/json" } });
        if (!response.ok) throw new Error(`Timeline unavailable (${response.status})`);
        return await response.json();
      }, (series) => {
        this.series = series;
        this.setAttribute("data-drawn", "");
        this.draw();
      }).catch(report);
    }
    draw() {
      if (!this.canvas || !this.series) return;
      this.drawn = layout(this.series, this.window, this.plotWidth());
      const style = getComputedStyle(this);
      const colour = (name) => style.getPropertyValue(name).trim();
      drawTimeline(
        this.canvas,
        this.drawn,
        this.window,
        this.dataset.timeZone ?? "UTC",
        this.focused,
        this.selection,
        { ink: colour("--ink"), muted: colour("--muted-ink"), line: colour("--line"), accent: colour("--accent"), font: colour("--font-data") }
      );
    }
    /** Draw the current series over the new window at once; fetch its own series shortly after. */
    show(window2) {
      this.dataset.start = String(window2.start);
      this.dataset.end = String(window2.end);
      this.draw();
      this.dispatchEvent(new CustomEvent("shell:window", { bubbles: true, detail: window2 }));
      clearTimeout(this.fetchLater);
      this.fetchLater = setTimeout(() => this.load(), 150);
    }
    describe(shape) {
      if (!this.description) return;
      if (!shape) {
        this.description.textContent = "";
        return;
      }
      const row = shape.series, format = new Intl.DateTimeFormat("en", { timeZone: this.dataset.timeZone ?? "UTC", dateStyle: "medium", timeStyle: "short" });
      const what = row.shape.render === "count" ? `${row.shape.count} rows \xB7 Enter zooms in` : row.shape.label ?? row.shape.reference;
      this.description.textContent = `${this.drawn?.rows[shape.row]?.label ?? row.lane} \xB7 ${what} \xB7 ${format.format(row.start)}`;
    }
    stepShape(direction, edge = false) {
      if (!this.drawn) return;
      const shape = step(this.drawn, edge ? null : this.focused, direction);
      this.focused = shape?.index ?? null;
      this.draw();
      this.describe(shape);
    }
    panBy(fraction) {
      this.show(pan(this.window, fraction));
    }
    zoomBy(factor, fraction = 0.5) {
      this.show(zoom(this.window, factor, fraction));
    }
    openShape() {
      const row = this.focused === null ? null : this.series?.rows[this.focused];
      if (!row) return;
      const inside = countWindow(row);
      if (inside) {
        this.focused = null;
        this.show(inside);
        return;
      }
      if (row.shape.render !== "count") location.assign(url(row.shape.href).href);
    }
    fraction(event) {
      const box = this.canvas?.getBoundingClientRect();
      return box ? (event.clientX - box.left - LABEL_WIDTH) / this.plotWidth() : 0.5;
    }
    pointerDown(event) {
      this.canvas?.setPointerCapture(event.pointerId);
      this.touches.set(event.pointerId, event.clientX);
      this.dragFrom = this.touches.size === 1 ? timeOf(this.fraction(event) * this.plotWidth(), this.window, this.plotWidth()) : null;
    }
    pointerMove(event) {
      const previous = this.touches.get(event.pointerId);
      if (previous === void 0) return;
      if (this.touches.size === 2) {
        const [first, second] = [...this.touches.values()];
        this.touches.set(event.pointerId, event.clientX);
        const [nextFirst, nextSecond] = [...this.touches.values()];
        if (first === void 0 || second === void 0 || nextFirst === void 0 || nextSecond === void 0) return;
        const before = Math.abs(first - second), after = Math.abs(nextFirst - nextSecond);
        if (before > 0 && after > 0) this.zoomBy(after / before, this.fraction({ clientX: (nextFirst + nextSecond) / 2 }));
        return;
      }
      this.touches.set(event.pointerId, event.clientX);
      if (this.dragFrom === null) return;
      const to = timeOf(this.fraction(event) * this.plotWidth(), this.window, this.plotWidth());
      this.selection = { start: Math.min(this.dragFrom, to), end: Math.max(this.dragFrom, to) };
      this.draw();
    }
    pointerUp(event) {
      this.touches.delete(event.pointerId);
      const selection = this.selection, from = this.dragFrom;
      this.dragFrom = null;
      if (from === null) return;
      const pixels = selection ? (selection.end - selection.start) / (this.window.end - this.window.start) * this.plotWidth() : 0;
      if (!selection || pixels < 3) {
        this.selection = null;
        const box = this.canvas?.getBoundingClientRect();
        const shape = this.drawn && box ? shapeAt(this.drawn, event.clientX - box.left - LABEL_WIDTH, event.clientY - box.top - AXIS_HEIGHT) : null;
        this.focused = shape?.index ?? null;
        this.draw();
        this.describe(shape);
        this.focus();
        return;
      }
      const window2 = { start: Math.round(selection.start), end: Math.round(selection.end) };
      this.dispatchEvent(new CustomEvent("shell:select", { bubbles: true, detail: window2 }));
      const rowsTarget = this.dataset.rows, rowsRoute = this.dataset.rowsRoute;
      if (rowsTarget && rowsRoute) {
        const address = url(rowsRoute);
        address.searchParams.set("start", String(window2.start));
        address.searchParams.set("end", String(window2.end));
        void fragments?.receive({ kind: "ref", target: rowsTarget, url: address.href }).catch(report);
      }
    }
    wheel(event) {
      if (event.ctrlKey) {
        event.preventDefault();
        this.zoomBy(Math.exp(Math.max(-4, Math.min(4, -event.deltaY / 500))), this.fraction(event));
        return;
      }
      const sideways = event.shiftKey ? event.deltaY : Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : 0;
      if (sideways) {
        event.preventDefault();
        this.panBy(sideways / this.plotWidth());
      }
    }
  };
}
function previewElement() {
  return class ShellPreview extends HTMLElement {
    timer;
    panel = null;
    connectedCallback() {
      const show = () => {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => void this.show().catch(report), 350);
      };
      const hide = () => {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => this.hide(), 150);
      };
      this.addEventListener("pointerenter", show);
      this.addEventListener("focusin", show);
      this.addEventListener("pointerleave", hide);
      this.addEventListener("focusout", hide);
    }
    disconnectedCallback() {
      clearTimeout(this.timer);
      this.hide();
    }
    async show() {
      const route = this.dataset.route;
      if (!route) return;
      await requests.run(`preview:${route}`, async (signal) => {
        const response = await fetch(url(route), { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
        if (!response.ok) throw new Error(`Preview unavailable (${response.status})`);
        return await response.text();
      }, (html) => {
        const template = document.createElement("template");
        template.innerHTML = html;
        if (activeContent(template.content)) throw new Error("Preview contains unsupported active content");
        const panel = this.panel ?? document.createElement("div");
        panel.className = "preview-panel";
        panel.setAttribute("popover", "manual");
        panel.replaceChildren(template.content);
        if (!this.panel) {
          this.append(panel);
          this.panel = panel;
        }
        const box = this.getBoundingClientRect();
        panel.style.left = `${Math.max(8, Math.min(box.left, innerWidth - 400))}px`;
        panel.style.top = `${box.bottom + 4}px`;
        panel.showPopover();
      });
    }
    hide() {
      if (this.panel?.matches(":popover-open")) this.panel.hidePopover();
    }
  };
}
function defineElements() {
  const define = (name, constructor) => {
    if (!customElements.get(name)) customElements.define(name, constructor);
  };
  define("shell-window", class extends HTMLElement {
    started = false;
    connectedCallback() {
      if (!this.started) {
        this.started = true;
        enhanceWindow(this);
      }
    }
  });
  define("shell-board", class extends HTMLElement {
    started = false;
    connectedCallback() {
      if (!this.started) {
        this.started = true;
        boardPointers(this);
      }
    }
  });
  define("shell-copy", class extends HTMLElement {
    started = false;
    connectedCallback() {
      if (this.started) return;
      this.started = true;
      one("[data-copy-menu]", this)?.addEventListener("click", (event) => {
        event.preventDefault();
        try {
          copyMenu(this);
        } catch (error) {
          report(error);
        }
      });
    }
  });
  define("shell-feed", class extends HTMLElement {
    connectedCallback() {
      feeds.add(this);
      if (!document.hidden) void follow(this, false, visibilityGeneration).catch(report);
    }
    disconnectedCallback() {
      feeds.delete(this);
      unfollow(this, true);
    }
  });
  define("shell-card", class extends HTMLElement {
    started = false;
    connectedCallback() {
      if (!this.hasAttribute("tabindex")) this.tabIndex = -1;
      if (this.started) return;
      this.started = true;
      this.addEventListener("focusout", (event) => {
        if (!(event.relatedTarget instanceof Node && this.contains(event.relatedTarget))) this.removeAttribute("data-entered");
      });
    }
    disconnectedCallback() {
      this.removeAttribute("data-entered");
    }
  });
  define("shell-editor", class extends HTMLElement {
    started = false;
    connectedCallback() {
      if (this.started) return;
      this.started = true;
      this.addEventListener("keydown", (event) => {
        if (event.key !== "Enter" || !event.ctrlKey) return;
        const form = event.target.closest("form") ?? one("form", this);
        if (form) {
          event.preventDefault();
          form.requestSubmit();
        }
      });
    }
  });
  define("shell-seen", class extends HTMLElement {
    mark = null;
    intersecting = false;
    observer = null;
    check = () => {
      void this.mark?.update(this.state());
    };
    state() {
      return {
        visibilityState: document.visibilityState,
        prerendering: document.prerendering === true,
        intersecting: this.intersecting,
        inPreview: Boolean(this.closest("shell-preview")),
        rendered: this.checkVisibility?.() ?? true
      };
    }
    connectedCallback() {
      const action = JSON.parse(this.dataset.action ?? "null");
      if (!action) return;
      const fallback = one("[data-action-item]", this);
      if (fallback) fallback.hidden = true;
      this.mark = new SeenMark(() => submit(action), (error) => {
        if (fallback) fallback.hidden = false;
        report(error);
      });
      this.observer = new IntersectionObserver((entries) => {
        this.intersecting = entries.some((entry) => entry.isIntersecting);
        this.check();
      });
      this.observer.observe(this);
      document.addEventListener("visibilitychange", this.check);
      document.addEventListener("prerenderingchange", this.check);
    }
    disconnectedCallback() {
      this.observer?.disconnect();
      this.observer = null;
      this.mark = null;
      document.removeEventListener("visibilitychange", this.check);
      document.removeEventListener("prerenderingchange", this.check);
    }
  });
  define("shell-timeline", timelineElement());
  define("shell-preview", previewElement());
}
function boot() {
  document.addEventListener("focusin", (event) => {
    const row = target(event)?.closest("[data-row]");
    if (row) lastFocusedRow = row;
  });
  document.addEventListener("keydown", keydown);
  document.addEventListener("click", (event) => {
    const element = target(event);
    if (element?.closest("[data-open-palette]")) openDialog("palette");
    if (element?.closest("[data-open-help]")) {
      const help = one("#key-help");
      if (help) help.open = !help.open;
    }
    if (element?.closest("[data-close-dialog]")) element.closest("dialog")?.close();
    const action = element?.closest("[data-action]");
    if (action?.dataset.action) {
      event.preventDefault();
      void runAction(JSON.parse(action.dataset.action)).catch(report);
    }
    const follow2 = element?.closest("[data-toggle-follow]");
    if (follow2) {
      const reader = document.getElementById(follow2.dataset.toggleFollow ?? "");
      if (reader) {
        reader.dataset.follow = reader.dataset.follow === "true" ? "false" : "true";
        follow2.setAttribute("aria-pressed", String(reader.dataset.follow === "true"));
        if (reader.dataset.follow === "true") fragments?.flush(reader.id);
      }
    }
  });
  one("#palette-query")?.addEventListener("input", paletteFilter);
  const query = one("[data-query]");
  query?.addEventListener("input", () => {
    if (!query.dataset.completion) return;
    const address = url(query.dataset.completion);
    address.searchParams.set("q", query.value);
    address.searchParams.set("position", String(query.selectionStart ?? query.value.length));
    void requests.run("completion", async (signal) => {
      const response = await fetch(address, { signal, credentials: "same-origin" });
      if (!response.ok) throw new Error(`Completion unavailable (${response.status})`);
      return await response.json();
    }, (completions) => {
      const list = one("#query-completions");
      list?.replaceChildren(...completions.map((completion) => {
        const option = document.createElement("option");
        option.value = completion.value;
        option.label = completion.label;
        return option;
      }));
    }).catch(report);
  });
  one("[data-shortcuts]")?.addEventListener("change", (event) => {
    document.body.dataset.shortcuts = String(event.target.checked);
  });
  const narrow = matchMedia("(max-width: 899px)");
  const rails = () => all(".rail").forEach((rail) => {
    rail.open = !narrow.matches;
  });
  rails();
  narrow.addEventListener("change", rails);
  document.addEventListener("visibilitychange", () => {
    void visibility(true).catch(report);
  });
  window.addEventListener("pagehide", () => {
    for (const element of feeds) unfollow(element, false);
    hubPort?.postMessage({ kind: "leave" });
    requests.abortAll();
  });
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) void visibility(true).catch(report);
  });
  defineElements();
}
if (typeof document !== "undefined") {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot, { once: true });
  else boot();
}
export {
  FragmentController,
  SeenMark,
  SubscriptionSource,
  bindFeed,
  boardPointers,
  copyTarget,
  deliver,
  displayed,
  enhanceWindow,
  keydown,
  presentSnapshot,
  protectedTarget,
  pruneWindow,
  resolveKey
};
