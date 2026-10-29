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
var SHARED_KEYS = [{ "key": "/", "prefix": null, "ctrl": false, "action": "focus_query", "picker": null, "label": "Focus query" }, { "key": ":", "prefix": null, "ctrl": false, "action": "palette", "picker": null, "label": "Actions palette" }, { "key": "k", "prefix": null, "ctrl": true, "action": "palette", "picker": null, "label": "Actions palette" }, { "key": "?", "prefix": null, "ctrl": false, "action": "help", "picker": null, "label": "Key help" }, { "key": "g", "prefix": null, "ctrl": false, "action": "go_prefix", "picker": null, "label": "Go to section (then a letter)" }, { "key": "j", "prefix": null, "ctrl": false, "action": "next", "picker": "Ctrl-j", "label": "Next item" }, { "key": "ArrowDown", "prefix": null, "ctrl": false, "action": "next", "picker": "ArrowDown", "label": "Next item" }, { "key": "k", "prefix": null, "ctrl": false, "action": "previous", "picker": "Ctrl-k", "label": "Previous item" }, { "key": "ArrowUp", "prefix": null, "ctrl": false, "action": "previous", "picker": "ArrowUp", "label": "Previous item" }, { "key": "g", "prefix": "g", "ctrl": false, "action": "first", "picker": null, "label": "First item" }, { "key": "G", "prefix": null, "ctrl": false, "action": "last", "picker": "Ctrl-Shift-g", "label": "Last item" }, { "key": "Enter", "prefix": null, "ctrl": false, "action": "open", "picker": "Enter", "label": "Open focused item" }, { "key": "x", "prefix": null, "ctrl": false, "action": "toggle_selection", "picker": "Ctrl-x", "label": "Toggle selection" }, { "key": "*", "prefix": null, "ctrl": false, "action": "select_visible", "picker": null, "label": "Select visible items" }, { "key": "y", "prefix": null, "ctrl": false, "action": "copy_menu", "picker": "Ctrl-y", "label": "Copy menu" }, { "key": "Y", "prefix": null, "ctrl": false, "action": "copy_default", "picker": "Ctrl-Shift-y", "label": "Copy default form" }, { "key": "c", "prefix": "y", "ctrl": false, "action": "copy_command", "picker": null, "label": "Copy CLI command" }, { "key": "n", "prefix": null, "ctrl": false, "action": "next_match", "picker": "Ctrl-n", "label": "Next find match" }, { "key": "N", "prefix": null, "ctrl": false, "action": "previous_match", "picker": "Ctrl-Shift-n", "label": "Previous find match" }, { "key": "Escape", "prefix": null, "ctrl": false, "action": "close", "picker": "Escape", "label": "Close and clear selection" }];
var RELOAD_EVENT = "reload";

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
    if (template.content.querySelector("script,style,[style],iframe,object,embed")) throw new Error("Actions contain unsupported active content");
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
  apply(element, update, follow) {
    const reader = element.closest("[data-follow]") ?? element;
    const scroll = reader.scrollTop;
    const template = this.doc.createElement("template");
    template.innerHTML = update.html;
    if (template.content.querySelector("script,style,[style],iframe,object,embed")) {
      status("Fragment contains unsupported active content.");
      return;
    }
    if (update.kind === "append") element.append(template.content);
    else element.replaceChildren(template.content);
    reader.scrollTop = follow ? reader.scrollHeight : scroll;
    enhanceWindows(element);
    all("[data-pointer]", element).forEach(pointers);
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
var stream;
var closeStream = () => {
};
var feedRequiresReload = () => false;
var hadStream = false;
var visibilityGeneration = 0;
async function visibility() {
  const generation = ++visibilityGeneration;
  closeStream();
  stream = void 0;
  if (document.hidden) {
    requests.abortAll();
    return;
  }
  if (feedRequiresReload()) return;
  const feed = document.body.dataset.feed;
  const refresh = document.body.dataset.refresh;
  if (hadStream && refresh) {
    let refreshed;
    const applied = await requests.run("refresh", async (signal) => {
      const response = await fetch(url(refresh), { signal, credentials: "same-origin", headers: { Accept: "application/json" } });
      if (!response.ok) throw new Error(`Refresh unavailable (${response.status})`);
      return await response.json();
    }, (snapshot) => {
      if (!Array.isArray(snapshot.fragments) || typeof snapshot.position !== "string") throw new Error("Refresh must contain fragments and their read position");
      refreshed = snapshot;
    });
    if (!applied || !refreshed || generation !== visibilityGeneration || document.hidden) return;
    const presented = await presentSnapshot(
      refreshed,
      (update) => fragments?.receive(update) ?? Promise.resolve(),
      () => generation === visibilityGeneration && !document.hidden,
      (position2) => {
        document.body.dataset.position = position2;
      }
    );
    if (!presented) return;
  }
  if (!feed || generation !== visibilityGeneration || document.hidden) return;
  const address = url(feed);
  const position = document.body.dataset.position;
  if (position) address.searchParams.set("since", position);
  stream = new EventSource(address, { withCredentials: true });
  hadStream = true;
  const bound = bindFeed(stream, (update) => fragments?.receive(update) ?? Promise.resolve(), (position2) => {
    document.body.dataset.position = position2;
  });
  closeStream = bound.close;
  feedRequiresReload = bound.requiresReload;
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
  source.addEventListener("fragment", (event) => {
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
async function runAction(action) {
  const reason = "reason" in action.availability ? action.availability.reason : action.availability.state;
  if (!["enabled", "partial"].includes(action.availability.state)) {
    status(`${action.label}: ${reason}`);
    return;
  }
  const address = url(action.route);
  if (!action.write) {
    const request = JSON.parse(action.request);
    for (const [key, value] of Object.entries(request.arguments)) {
      if (value !== null && value !== void 0) address.searchParams.set(key, typeof value === "string" ? value : JSON.stringify(value));
    }
    location.assign(address.href);
    return;
  }
  await requests.run(`operation:${action.operation}`, async (signal) => {
    const response = await fetch(address, { method: "POST", signal, credentials: "same-origin", headers: { "Content-Type": "application/json", Accept: "text/html" }, body: action.request });
    if (!response.ok) throw new Error(`${action.label} failed (${response.status}). Editor text is retained.`);
    return await response.text();
  }, (html) => {
    status(`${action.label} completed`);
    if (html.trimStart().startsWith("<")) {
      const selected = heldRow ?? focusedRow();
      if (selected?.id) void fragments?.receive({ kind: "replace", target: selected.id, html }).catch(report);
    }
    dialog("palette")?.close();
  });
}
var prefix = "";
var prefixExpires = 0;
function resolveKey(key, ctrl, prefix2 = "") {
  return SHARED_KEYS.find((binding) => binding.key === key && binding.ctrl === ctrl && (binding.prefix ?? "") === prefix2)?.action ?? null;
}
function keydown(event) {
  const element = target(event);
  if (event.isComposing || event.altKey || event.metaKey) return;
  if (resolveKey(event.key, event.ctrlKey) === "close") {
    const activeDialog = element?.closest("dialog");
    activeDialog?.close();
    const disclosure = element?.closest("details");
    if (disclosure) {
      disclosure.open = false;
      one("summary", disclosure)?.focus();
    }
    for (const row of all('[aria-selected="true"]')) row.setAttribute("aria-selected", "false");
    element?.closest("[data-editor]")?.setAttribute("hidden", "");
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
  const keyAction = resolveKey(event.key, event.ctrlKey, wasPrefix) ?? directAction;
  if (resolveKey(wasPrefix, false) === "go_prefix" && keyAction !== "first") {
    const link = all("[data-go]").find((link2) => link2.dataset.go === event.key);
    if (link) {
      event.preventDefault();
      link.click();
      return;
    }
  }
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
    case "next":
      event.preventDefault();
      focusRow(index + 1, candidates);
      break;
    case "previous":
      event.preventDefault();
      focusRow(index < 0 ? candidates.length - 1 : index - 1, candidates);
      break;
    case "first":
      event.preventDefault();
      focusRow(0, candidates);
      break;
    case "copy_command":
      event.preventDefault();
      copyCommand();
      break;
    case "last":
      event.preventDefault();
      focusRow(candidates.length - 1, candidates);
      break;
    case "open":
      if (element === focusedRow()) {
        event.preventDefault();
        openRow();
      }
      break;
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
var windowed = /* @__PURE__ */ new WeakSet();
function enhanceWindows(root = document) {
  for (const list of all("[data-window]", root)) {
    if (windowed.has(list)) continue;
    windowed.add(list);
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
      if (template.content.querySelector("script,style,[style],iframe,object,embed")) throw new Error("Window contains unsupported active content");
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
  const maximum = Math.max(1, Number(list.dataset.window) || 200);
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
var pointerTargets = /* @__PURE__ */ new WeakSet();
function pointers(element) {
  if (pointerTargets.has(element)) return;
  pointerTargets.add(element);
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
    element.dispatchEvent(new CustomEvent("shell:pointer", { bubbles: true, detail: { kind: element.dataset.pointer, start, end, deltaY, phase } }));
    if (phase === "commit" && element.dataset.pointerRoute && element.id) {
      const address = url(element.dataset.pointerRoute);
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
    if (element.dataset.pointer === "board") {
      element.scrollLeft -= deltaX;
      element.scrollTop -= deltaY;
    }
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
    const source = element?.closest("[data-copy-menu]")?.closest("[data-copy]");
    if (source) {
      event.preventDefault();
      try {
        copyMenu(source);
      } catch (error) {
        report(error);
      }
    }
    const action = element?.closest("[data-action]");
    if (action?.dataset.action) {
      event.preventDefault();
      void runAction(JSON.parse(action.dataset.action)).catch(report);
    }
    const follow = element?.closest("[data-toggle-follow]");
    if (follow) {
      const reader = document.getElementById(follow.dataset.toggleFollow ?? "");
      if (reader) {
        reader.dataset.follow = reader.dataset.follow === "true" ? "false" : "true";
        follow.setAttribute("aria-pressed", String(reader.dataset.follow === "true"));
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
  enhanceWindows();
  all("[data-pointer]").forEach(pointers);
  document.addEventListener("visibilitychange", () => {
    void visibility().catch(report);
  });
  window.addEventListener("pagehide", () => {
    closeStream();
    requests.abortAll();
  });
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) void visibility().catch(report);
  });
  void visibility().catch(report);
}
if (typeof document !== "undefined") {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot, { once: true });
  else boot();
}
export {
  FragmentController,
  bindFeed,
  copyTarget,
  keydown,
  pointers,
  presentSnapshot,
  protectedTarget,
  pruneWindow,
  resolveKey
};
