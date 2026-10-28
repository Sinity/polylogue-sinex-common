import { LatestRequest, shouldHold, nearEnd, matchScore, pointerRange, sameOrigin, copyText, windowTrim } from "./core";
import { SHARED_KEYS } from "./types";
import type { Action, Completion, CopyTarget, FragmentUpdate, Refresh, KeyAction } from "./types";

const requests = new LatestRequest();
function status(message: string): void { const line = document.getElementById("shell-status"); if (line) line.textContent = message; }
function report(error: unknown): void { status(error instanceof Error ? error.message : String(error)); }
function url(value: string): URL { return sameOrigin(value, location.href); }
function one<T extends Element = HTMLElement>(selector: string, root: ParentNode = document): T | null { return root.querySelector<T>(selector); }
function all<T extends Element = HTMLElement>(selector: string, root: ParentNode = document): T[] { return Array.from(root.querySelectorAll<T>(selector)); }
function target(event: Event): HTMLElement | null { return event.target instanceof Element ? event.target.closest<HTMLElement>("*") : null; }
function dialog(id: string): HTMLDialogElement | null { return document.getElementById(id) as HTMLDialogElement | null; }
let lastFocusedRow: HTMLElement | null = null;
let heldRow: HTMLElement | null = null;
function openDialog(id: string): void {
  const element = dialog(id);
  if (!element || element.open) return;
  heldRow = focusedRow() ?? lastFocusedRow;
  element.addEventListener("close", () => { heldRow = null; requests.cancel("palette"); }, { once: true });
  element.showModal();
  if (id === "palette") void refreshActions().catch(report);
}
async function refreshActions(): Promise<void> {
  const route = heldRow?.dataset.actionsRoute ?? document.body.dataset.actionsRoute;
  if (!route) return;
  const references = all<HTMLElement>('[data-row][aria-selected="true"]').map(row => row.dataset.reference ?? row.id);
  if (!references.length && heldRow) references.push(heldRow.dataset.reference ?? heldRow.id);
  const address = url(route); address.searchParams.set("references", JSON.stringify(references));
  await requests.run("palette", async signal => {
    const response = await fetch(address, { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
    if (!response.ok) throw new Error(`Actions unavailable (${response.status})`);
    return response.text();
  }, html => {
    const container = one<HTMLElement>("[data-palette-actions]");
    const template = document.createElement("template"); template.innerHTML = html;
    if (template.content.querySelector("script,style,[style],iframe,object,embed")) throw new Error("Actions contain unsupported active content");
    container?.replaceChildren(template.content); paletteFilter();
  });
}
function selectedText(): boolean { return Boolean(document.getSelection()?.toString()); }
function protectedTarget(element: HTMLElement): boolean {
  const active = document.activeElement;
  return Boolean(heldRow && (element === heldRow || element.contains(heldRow))) || (active !== document.body && active !== element && Boolean(active && element.contains(active))) || active === element ||
    Boolean(one('[aria-selected="true"]', element)) ||
    Boolean(selectedText() && document.getSelection()?.containsNode(element, true));
}
function rows(root: ParentNode = document): HTMLElement[] { return all<HTMLElement>("[data-row]", root).filter(row => !row.hidden); }
function focusedRow(): HTMLElement | null { return document.activeElement?.closest<HTMLElement>("[data-row]") ?? null; }
function focusRow(index: number, candidates = rows()): void {
  const row = candidates[Math.max(0, Math.min(index, candidates.length - 1))];
  if (!row) return;
  if (!row.hasAttribute("tabindex")) row.tabIndex = -1;
  row.focus({ preventScroll: true }); row.scrollIntoView({ block: "nearest" });
}
function toggleRow(row: HTMLElement | null): void { if (row) row.setAttribute("aria-selected", row.getAttribute("aria-selected") === "true" ? "false" : "true"); }
function openRow(): void { const row = focusedRow(); one<HTMLAnchorElement>("a[data-open], a", row ?? document)?.click(); }

/** A pending update belongs to a target and never replaces a focused/selected tree. */
export class FragmentController {
  private pending = new Map<string, FragmentUpdate[]>();
  constructor(private readonly doc: Document = document) {}
  receive(update: FragmentUpdate): void {
    const element = this.doc.getElementById(update.target);
    if (!(element instanceof HTMLElement)) return;
    if (update.kind === "ref") {
      void requests.run(`fragment:${update.target}`, async signal => {
        const response = await fetch(url(update.url), { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
        if (!response.ok) throw new Error(`Fragment unavailable (${response.status})`);
        return await response.text();
      }, html => this.receive({ kind: "replace", target: update.target, html })).catch(report);
      return;
    }
    requests.cancel(`fragment:${update.target}`);
    const reader = element.closest<HTMLElement>("[data-follow]") ?? element;
    const state = { following: reader.dataset.follow === "true", atEnd: nearEnd(reader.scrollTop, reader.clientHeight, reader.scrollHeight), protected: protectedTarget(element) };
    if (this.pending.has(update.target) || shouldHold(update.kind, state)) {
      const pending = this.pending.get(update.target) ?? [];
      // A full replacement supersedes older replacements but never drops appends.
      if (update.kind === "replace") {
        const previous = pending.findIndex(item => item.kind === "replace");
        if (previous >= 0) pending.splice(previous, 1);
      }
      pending.push(update); this.pending.set(update.target, pending);
      this.notice(element, pending.length, update.kind === "append");
    } else this.apply(element, update, state.following && state.atEnd);
  }
  private notice(element: HTMLElement, count: number, append: boolean): void {
    let button = all<HTMLButtonElement>("[data-updates]").find(button => button.dataset.updates === element.id);
    if (!button) {
      button = this.doc.createElement("button"); button.type = "button"; button.dataset.updates = element.id;
      button.addEventListener("click", () => this.flush(element.id)); element.after(button);
    }
    button.textContent = append ? `${count} new ↓` : `${count} updates · apply`;
  }
  flush(id: string): void {
    const element = this.doc.getElementById(id);
    if (!(element instanceof HTMLElement)) return;
    // Explicit apply preserves protected rows and leaves the notice until focus/selection leaves.
    if (protectedTarget(element)) { status("Move focus out of the updated item and clear its selection to apply updates."); return; }
    for (const update of this.pending.get(id) ?? []) {
      if (update.kind !== "ref") this.apply(element, update, update.kind === "append");
    }
    this.pending.delete(id);
    all<HTMLElement>("[data-updates]").find(button => button.dataset.updates === id)?.remove();
  }
  private apply(element: HTMLElement, update: Exclude<FragmentUpdate, { kind: "ref" }>, follow: boolean): void {
    const reader = element.closest<HTMLElement>("[data-follow]") ?? element;
    const scroll = reader.scrollTop;
    const template = this.doc.createElement("template"); template.innerHTML = update.html;
    if (template.content.querySelector("script,style,[style],iframe,object,embed")) { status("Fragment contains unsupported active content."); return; }
    if (update.kind === "append") element.append(template.content);
    else element.replaceChildren(template.content);
    reader.scrollTop = follow ? reader.scrollHeight : scroll;
    enhanceWindows(element);
    all<HTMLElement>("[data-pointer]", element).forEach(pointers);
  }
}
const fragments = typeof document === "undefined" ? null : new FragmentController();
function readUpdate(value: unknown): FragmentUpdate {
  if (typeof value !== "object" || value === null) throw new Error("Invalid fragment event");
  const item = value as Record<string, unknown>;
  if (typeof item.target !== "string" || !["append", "replace", "ref"].includes(String(item.kind))) throw new Error("Invalid fragment event");
  if (item.kind === "ref" ? typeof item.url !== "string" : typeof item.html !== "string") throw new Error("Invalid fragment payload");
  return item as unknown as FragmentUpdate;
}
let stream: EventSource | undefined;
let hadStream = false;
let visibilityGeneration = 0;
async function visibility(): Promise<void> {
  const generation = ++visibilityGeneration;
  stream?.close(); stream = undefined;
  if (document.hidden) { requests.abortAll(); return; }
  const feed = document.body.dataset.feed;
  const refresh = document.body.dataset.refresh;
  if (hadStream && refresh) {
    await requests.run("refresh", async signal => {
      const response = await fetch(url(refresh), { signal, credentials: "same-origin", headers: { Accept: "application/json" } });
      if (!response.ok) throw new Error(`Refresh unavailable (${response.status})`);
      return await response.json() as Refresh;
    }, snapshot => {
      if (!Array.isArray(snapshot.fragments) || typeof snapshot.position !== "string") throw new Error("Refresh must contain fragments and their read position");
      for (const value of snapshot.fragments) fragments?.receive(readUpdate(value));
      document.body.dataset.position = snapshot.position;
    });
  }
  if (!feed || generation !== visibilityGeneration || document.hidden) return;
  const address = url(feed);
  const position = document.body.dataset.position;
  // Native EventSource supplies Last-Event-ID on automatic reconnect. A visibility
  // close creates a new connection, so the snapshot position travels in `since`.
  if (position) address.searchParams.set("since", position);
  stream = new EventSource(address, { withCredentials: true }); hadStream = true;
  stream.addEventListener("fragment", event => {
    try {
      fragments?.receive(readUpdate(JSON.parse(event.data)));
      if (event.lastEventId) document.body.dataset.position = event.lastEventId;
    } catch (error) { report(error); }
  });
  stream.addEventListener("gap", event => { stream?.close(); status(`Subscription gap: ${event.data}. Reload the view.`); });
  stream.onopen = () => status("");
  stream.onerror = () => status("Disconnected · retrying");
}

let copySource: HTMLElement | null = null;
export async function copyTarget(source: HTMLElement, formIndex: number): Promise<void> {
  const value = JSON.parse(source.dataset.copy ?? "null") as CopyTarget | null;
  const form = value?.forms[formIndex]; if (!value || !form) return;
  const result = await copyText(form.text, navigator.clipboard);
  const feedback = one<HTMLElement>("[data-copy-feedback]", source) ?? source;
  source.querySelector(".copy-fallback")?.remove();
  if (result.copied) feedback.textContent = `${value.label} as ${form.label} copied · ${result.characters.toLocaleString()} characters`;
  else {
    feedback.textContent = "Clipboard unavailable. Select and copy the text below.";
    const box = document.createElement("textarea"); box.className = "copy-fallback"; box.readOnly = true; box.value = form.text;
    box.setAttribute("aria-label", `${value.label} as ${form.label}`); source.append(box); box.focus(); box.select();
  }
}
function copyMenu(source: HTMLElement): void {
  copySource = source;
  const value = JSON.parse(source.dataset.copy ?? "null") as CopyTarget | null;
  const forms = one<HTMLElement>("[data-copy-forms]"); if (!forms || !value) return;
  forms.replaceChildren();
  value.forms.forEach((form, index) => {
    const button = document.createElement("button"); button.type = "button"; button.textContent = form.label;
    button.addEventListener("click", () => { dialog("copy-menu")?.close(); if (copySource) void copyTarget(copySource, index).catch(report); }); forms.append(button);
  });
  openDialog("copy-menu");
}
function currentCopy(): HTMLElement | null { return focusedRow()?.closest<HTMLElement>("[data-copy]") ?? one<HTMLElement>("[data-copy]", focusedRow() ?? document); }
function copyCommand(): void { dialog("copy-menu")?.close(); const source = document.getElementById("page-command")?.closest<HTMLElement>("[data-copy]"); if (source) void copyTarget(source, 0).catch(report); }
function paletteFilter(): void {
  const query = one<HTMLInputElement>("#palette-query")?.value ?? "";
  for (const item of all<HTMLElement>("[data-action], [data-palette-navigation]", dialog("palette") ?? document)) {
    const action = item.dataset.action ? JSON.parse(item.dataset.action) as Action : null;
    const score = matchScore(query, `${item.textContent ?? ""} ${action?.cli ?? ""}`);
    const wrapper = item.closest<HTMLElement>("[data-action-item]") ?? item;
    wrapper.hidden = score === null;
  }
}
async function runAction(action: Action): Promise<void> {
  const reason = "reason" in action.availability ? action.availability.reason : action.availability.state;
  if (!["enabled", "partial"].includes(action.availability.state)) { status(`${action.label}: ${reason}`); return; }
  const address = url(action.route);
  if (!action.write) {
    const request = JSON.parse(action.request) as { arguments: Record<string, unknown> };
    for (const [key, value] of Object.entries(request.arguments)) {
      if (value !== null && value !== undefined) address.searchParams.set(key, typeof value === "string" ? value : JSON.stringify(value));
    }
    location.assign(address.href); return;
  }
  await requests.run(`operation:${action.operation}`, async signal => {
    const response = await fetch(address, { method: "POST", signal, credentials: "same-origin", headers: { "Content-Type": "application/json", Accept: "text/html" }, body: action.request });
    if (!response.ok) throw new Error(`${action.label} failed (${response.status}). Editor text is retained.`);
    return await response.text();
  }, html => {
    status(`${action.label} completed`);
    // The operation may return a server-rendered fragment; an opaque typed JSON answer
    // is never assembled into a browser view.
    if (html.trimStart().startsWith("<")) {
      const selected = heldRow ?? focusedRow();
      if (selected?.id) fragments?.receive({ kind: "replace", target: selected.id, html });
    }
    dialog("palette")?.close();
  });
}

let prefix = "";
let prefixExpires = 0;
export function resolveKey(key: string, ctrl: boolean, prefix: string = ""): KeyAction | null {
  return SHARED_KEYS.find(binding => binding.key === key && binding.ctrl === ctrl && (binding.prefix ?? "") === prefix)?.action ?? null;
}
function keydown(event: KeyboardEvent): void {
  const element = target(event);
  if (event.isComposing || event.altKey || event.metaKey) return;
  if (resolveKey(event.key, event.ctrlKey) === "close") {
    const activeDialog = element?.closest<HTMLDialogElement>("dialog"); activeDialog?.close();
    const disclosure = element?.closest<HTMLDetailsElement>("details"); if (disclosure) { disclosure.open = false; one<HTMLElement>("summary", disclosure)?.focus(); }
    for (const row of all<HTMLElement>('[aria-selected="true"]')) row.setAttribute("aria-selected", "false");
    element?.closest<HTMLElement>("[data-editor]")?.setAttribute("hidden", "");
    prefix = ""; return;
  }
  const directAction = resolveKey(event.key, event.ctrlKey);
  if (element?.id === "palette-query" && (directAction === "open" || directAction === "next")) {
    const item = all<HTMLElement>("[data-action], [data-palette-navigation]", dialog("palette") ?? document).find(item => !item.closest("[hidden]") && !item.hasAttribute("disabled"));
    if (item) { event.preventDefault(); if (directAction === "open") item.click(); else item.focus(); } return;
  }
  if (element?.closest("input,textarea,select,[contenteditable]:not([contenteditable=false])") || selectedText()) return;
  if (event.ctrlKey && directAction === "palette") { event.preventDefault(); openDialog("palette"); return; }
  if (event.ctrlKey) return;
  if (one<HTMLInputElement>("[data-shortcuts]")?.checked === false && event.key.length === 1) return;
  if (Date.now() > prefixExpires) prefix = "";
  const wasPrefix = prefix; prefix = "";
  const keyAction = resolveKey(event.key, event.ctrlKey, wasPrefix) ?? directAction;
  if (resolveKey(wasPrefix, false) === "go_prefix" && keyAction !== "first") {
    const link = all<HTMLAnchorElement>("[data-go]").find(link => link.dataset.go === event.key);
    if (link) { event.preventDefault(); link.click(); return; }
  }
  const palette = element?.closest<HTMLDialogElement>("#palette");
  const candidates = palette ? all<HTMLElement>("[data-action], [data-palette-navigation]", palette).filter(item => !item.closest("[hidden]") && !item.hasAttribute("disabled")) : rows();
  const index = candidates.indexOf((palette ? document.activeElement : focusedRow()) as HTMLElement);
  switch (keyAction) {
    case "focus_query": event.preventDefault(); one<HTMLInputElement>("[data-query]")?.focus(); break;
    case "palette": event.preventDefault(); openDialog("palette"); break;
    case "help": { event.preventDefault(); const help = one<HTMLDetailsElement>("#key-help"); if (help) help.open = !help.open; break; }
    case "go_prefix": prefix = event.key; prefixExpires = Date.now() + 1000; break;
    case "next": event.preventDefault(); focusRow(index + 1, candidates); break;
    case "previous": event.preventDefault(); focusRow(index < 0 ? candidates.length - 1 : index - 1, candidates); break;
    case "first": event.preventDefault(); focusRow(0, candidates); break;
    case "copy_command": event.preventDefault(); copyCommand(); break;
    case "last": event.preventDefault(); focusRow(candidates.length - 1, candidates); break;
    case "open": if (focusedRow()) { event.preventDefault(); openRow(); } break;
    case "toggle_selection": event.preventDefault(); toggleRow(focusedRow()); break;
    case "select_visible": event.preventDefault(); candidates.filter(row => { const rect = row.getBoundingClientRect(); return rect.bottom > 0 && rect.top < innerHeight; }).forEach(row => row.setAttribute("aria-selected", "true")); break;
    case "copy_menu": { event.preventDefault(); prefix = event.key; prefixExpires = Date.now() + 1000; const source = currentCopy(); if (source) copyMenu(source); break; }
    case "copy_default": { event.preventDefault(); const source = currentCopy(); if (source) void copyTarget(source, 0).catch(report); break; }
    case "next_match": case "previous_match": { event.preventDefault(); const matches = all<HTMLElement>("[data-find-match]"); const index = matches.findIndex(item => item === document.activeElement); focusRow((index + (keyAction === "next_match" ? 1 : -1) + matches.length) % matches.length, matches); break; }
    default: {
      const action = all<HTMLButtonElement>("[data-action]").find(button => {
        const action = JSON.parse(button.dataset.action ?? "null") as Action | null;
        return action?.key === event.key;
      }); action?.click();
    }
  }
}

const windowed = new WeakSet<HTMLElement>();
function enhanceWindows(root: ParentNode = document): void {
  for (const list of all<HTMLElement>("[data-window]", root)) {
    if (windowed.has(list)) continue;
    windowed.add(list);
    let previousScroll = list.scrollTop;
    list.addEventListener("scroll", () => {
      const direction = list.scrollTop >= previousScroll ? "after" : "before";
      previousScroll = list.scrollTop;
      const route = list.dataset.windowRoute ?? one<HTMLAnchorElement>("[data-window-before], [data-window-after]", list)?.href;
      if (route) pruneWindow(list, route, direction === "after" ? "before" : "after");
      if (list.scrollTop < list.clientHeight / 2) void windowPage(list, "before").catch(report);
      else if (list.scrollHeight - list.scrollTop - list.clientHeight < list.clientHeight / 2) void windowPage(list, "after").catch(report);
    }, { passive: true });
  }
}
const paging = new WeakSet<HTMLElement>();
async function windowPage(list: HTMLElement, direction: "before" | "after"): Promise<void> {
  if (paging.has(list)) return;
  const link = one<HTMLAnchorElement>(`[data-window-${direction}]`, list);
  if (!link) return;
  paging.add(list);
  try {
    const existing = rows(list);
    const anchor = direction === "before" ? existing[0] : existing.at(-1);
    if (!anchor?.dataset.anchor) return;
    const address = url(link.href); address.searchParams.set("anchor", anchor.dataset.anchor); address.searchParams.set("direction", direction);
    await requests.run(`window:${list.id}`, async signal => {
      const response = await fetch(address, { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
      if (!response.ok) throw new Error(`Window unavailable (${response.status})`);
      return await response.text();
    }, html => {
      const top = anchor.getBoundingClientRect().top;
      const template = document.createElement("template"); template.innerHTML = html;
      if (template.content.querySelector("script,style,[style],iframe,object,embed")) throw new Error("Window contains unsupported active content");
      const incoming = rows(template.content);
      const known = new Set(rows(list).map(row => row.dataset.anchor));
      for (const row of incoming) if (known.has(row.dataset.anchor)) row.remove();
      if (direction === "before") list.prepend(template.content); else list.append(template.content);
      link.remove();
      // Restore the old anchor before choosing visible rows to prune. In a prepend,
      // the incoming rows otherwise temporarily occupy the old viewport.
      list.scrollTop += anchor.getBoundingClientRect().top - top;
      list.dataset.windowRoute = link.href;
      pruneWindow(list, link.href, direction === "after" ? "before" : "after");
    });
  } finally { paging.delete(list); }
}

function pruneWindow(list: HTMLElement, route: string, preferred: "before" | "after"): void {
  const box = list.getBoundingClientRect();
  const candidates = rows(list).map(row => {
    const rect = row.getBoundingClientRect();
    return { key: row, visible: rect.bottom > box.top && rect.top < box.bottom, protected: protectedTarget(row) };
  });
  const anchor = candidates.find(row => row.visible)?.key;
  const top = anchor?.getBoundingClientRect().top;
  const maximum = Math.max(1, Number(list.dataset.window) || 200);
  const plan = windowTrim(candidates, maximum, preferred);
  plan.remove.forEach(row => row.remove());
  for (const edge of ["before", "after"] as const) {
    if (!plan[edge] || one(`[data-window-${edge}]`, list)) continue;
    const link = document.createElement("a"); link.href = route;
    link.dataset[edge === "before" ? "windowBefore" : "windowAfter"] = "";
    link.dataset.windowSentinel = ""; link.textContent = `Load ${edge}`;
    if (edge === "before") list.prepend(link); else list.append(link);
  }
  if (anchor && top !== undefined) list.scrollTop += anchor.getBoundingClientRect().top - top;
}

const pointerTargets = new WeakSet<HTMLElement>();
export function pointers(element: HTMLElement): void {
  if (pointerTargets.has(element)) return;
  pointerTargets.add(element);
  const positions = new Map<number, { x: number; y: number }>();
  let start = 0, end = 1, previousX = 0, previousDistance: number | undefined;
  const range = () => { start = Number(element.dataset.start ?? 0); end = Number(element.dataset.end ?? 1); };
  const publish = (phase: "preview" | "commit", deltaY = 0) => {
    element.dispatchEvent(new CustomEvent("shell:pointer", { bubbles: true, detail: { kind: element.dataset.pointer, start, end, deltaY, phase } }));
    if (phase === "commit" && element.dataset.pointerRoute && element.id) {
      const address = url(element.dataset.pointerRoute); address.searchParams.set("start", String(start)); address.searchParams.set("end", String(end));
      fragments?.receive({ kind: "ref", target: element.id, url: address.href });
    }
  };
  element.addEventListener("pointerdown", event => {
    if ((event.target as HTMLElement).closest("a,button,input,textarea,select,[contenteditable], [data-row]")) return;
    if (positions.size === 0) range(); positions.set(event.pointerId, { x: event.clientX, y: event.clientY }); previousX = event.clientX;
    element.setPointerCapture(event.pointerId);
  });
  element.addEventListener("pointermove", event => {
    const previous = positions.get(event.pointerId); if (!previous) return;
    positions.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const points = [...positions.values()];
    const first = points[0], second = points[1];
    const distance = first && second ? Math.hypot(first.x - second.x, first.y - second.y) : undefined;
    const scale = distance && previousDistance ? distance / previousDistance : 1;
    const box = element.getBoundingClientRect();
    [start, end] = pointerRange(start, end, box.width, event.clientX - previousX, scale, (event.clientX - box.left) / Math.max(1, box.width));
    if (element.dataset.pointer === "board") { element.scrollLeft -= event.clientX - previousX; element.scrollTop -= event.clientY - previous.y; }
    previousX = event.clientX; previousDistance = distance; publish("preview", event.clientY - previous.y);
  });
  const finish = (event: PointerEvent) => {
    if (!positions.has(event.pointerId)) return;
    positions.delete(event.pointerId); previousDistance = undefined;
    element.dataset.start = String(start); element.dataset.end = String(end); publish("commit");
  };
  element.addEventListener("pointerup", finish); element.addEventListener("pointercancel", finish);
  element.addEventListener("wheel", event => {
    if (!event.ctrlKey) return;
    event.preventDefault(); range();
    const box = element.getBoundingClientRect();
    [start, end] = pointerRange(start, end, box.width, 0, Math.exp(Math.max(-4, Math.min(4, -event.deltaY / 500))), (event.clientX - box.left) / Math.max(1, box.width));
    element.dataset.start = String(start); element.dataset.end = String(end); publish("commit");
  }, { passive: false });
}
function boot(): void {
  document.addEventListener("focusin", event => { const row = target(event)?.closest<HTMLElement>("[data-row]"); if (row) lastFocusedRow = row; });
  document.addEventListener("keydown", keydown);
  document.addEventListener("click", event => {
    const element = target(event);
    if (element?.closest("[data-open-palette]")) openDialog("palette");
    if (element?.closest("[data-open-help]")) { const help = one<HTMLDetailsElement>("#key-help"); if (help) help.open = !help.open; }
    if (element?.closest("[data-close-dialog]")) element.closest<HTMLDialogElement>("dialog")?.close();
    const source = element?.closest<HTMLElement>("[data-copy-menu]")?.closest<HTMLElement>("[data-copy]");
    if (source) { event.preventDefault(); try { copyMenu(source); } catch (error) { report(error); } }
    const action = element?.closest<HTMLElement>("[data-action]");
    if (action?.dataset.action) { event.preventDefault(); void runAction(JSON.parse(action.dataset.action) as Action).catch(report); }
    const follow = element?.closest<HTMLElement>("[data-toggle-follow]");
    if (follow) {
      const reader = document.getElementById(follow.dataset.toggleFollow ?? "");
      if (reader) { reader.dataset.follow = reader.dataset.follow === "true" ? "false" : "true"; follow.setAttribute("aria-pressed", String(reader.dataset.follow === "true")); if (reader.dataset.follow === "true") fragments?.flush(reader.id); }
    }
  });
  one<HTMLInputElement>("#palette-query")?.addEventListener("input", paletteFilter);
  const query = one<HTMLInputElement>("[data-query]");
  query?.addEventListener("input", () => {
    if (!query.dataset.completion) return;
    const address = url(query.dataset.completion); address.searchParams.set("q", query.value); address.searchParams.set("position", String(query.selectionStart ?? query.value.length));
    void requests.run("completion", async signal => {
      const response = await fetch(address, { signal, credentials: "same-origin" });
      if (!response.ok) throw new Error(`Completion unavailable (${response.status})`);
      return await response.json() as Completion[];
    }, completions => {
      const list = one<HTMLDataListElement>("#query-completions");
      list?.replaceChildren(...completions.map(completion => { const option = document.createElement("option"); option.value = completion.value; option.label = completion.label; return option; }));
    }).catch(report);
  });
  one<HTMLInputElement>("[data-shortcuts]")?.addEventListener("change", event => {
    // Preferences are view settings; no durable product state is held by the browser.
    document.body.dataset.shortcuts = String((event.target as HTMLInputElement).checked);
  });
  const narrow = matchMedia("(max-width: 899px)");
  const rails = () => all<HTMLDetailsElement>(".rail").forEach(rail => { rail.open = !narrow.matches; });
  rails(); narrow.addEventListener("change", rails);
  enhanceWindows(); all<HTMLElement>("[data-pointer]").forEach(pointers);
  document.addEventListener("visibilitychange", () => { void visibility().catch(report); });
  window.addEventListener("pagehide", () => { stream?.close(); requests.abortAll(); });
  window.addEventListener("pageshow", event => { if (event.persisted) void visibility().catch(report); });
  void visibility().catch(report);
}
if (typeof document !== "undefined") {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot, { once: true }); else boot();
}
