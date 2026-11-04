import { LatestRequest, shouldHold, nearEnd, matchScore, pointerRange, sameOrigin, copyText, windowTrim } from "./core";
import { FeedHub, browserTimer } from "./feed";
import type { HubMessage, PortLike, StreamEvent, StreamLike, TabMessage } from "./feed";
import { ROW_HEIGHT, countWindow, layout, pan, shapeAt, step, ticks, timeOf, zoom } from "./timeline";
import type { DrawnShape, Layout } from "./timeline";
import { SHARED_KEYS, RELOAD_EVENT, FRAGMENT_EVENT } from "./types";
import type { Action, Completion, CopyTarget, FragmentUpdate, Refresh, KeyAction, Series, Window } from "./types";

const requests = new LatestRequest();
function status(message: string): void { const line = document.getElementById("shell-status"); if (line) line.textContent = message; }
function report(error: unknown): void { status(error instanceof Error ? error.message : String(error)); }
function url(value: string): URL { return sameOrigin(value, location.href); }
function one<T extends Element = HTMLElement>(selector: string, root: ParentNode = document): T | null { return root.querySelector<T>(selector); }
function all<T extends Element = HTMLElement>(selector: string, root: ParentNode = document): T[] { return Array.from(root.querySelectorAll<T>(selector)); }
function target(event: Event): HTMLElement | null { return event.target instanceof Element ? event.target.closest<HTMLElement>("*") : null; }
function dialog(id: string): HTMLDialogElement | null { return document.getElementById(id) as HTMLDialogElement | null; }
function activeContent(fragment: ParentNode): boolean { return Boolean(fragment.querySelector("script,style,[style],iframe,object,embed")); }
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
    if (activeContent(template.content)) throw new Error("Actions contain unsupported active content");
    container?.replaceChildren(template.content); paletteFilter();
  });
}
function selectedText(): boolean { return Boolean(document.getSelection()?.toString()); }
export function protectedTarget(element: HTMLElement): boolean {
  const active = document.activeElement;
  return Boolean(heldRow && (element === heldRow || element.contains(heldRow))) || (active !== document.body && active !== element && Boolean(active && element.contains(active))) || active === element ||
    element.getAttribute("aria-selected") === "true" || Boolean(one('[aria-selected="true"]', element)) ||
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
  private revisions = new Map<string, number>();
  private resolving = new Map<string, Promise<void>>();
  constructor(private readonly doc: Document = document) {}
  receive(update: FragmentUpdate): Promise<void> {
    const element = this.doc.getElementById(update.target);
    if (!(element instanceof HTMLElement)) return Promise.resolve();
    let revision = this.revisions.get(update.target) ?? 0;
    if (update.kind !== "append") {
      // A later full snapshot includes preceding events for this target. It can
      // supersede an older snapshot, while appends after it must wait for it.
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
          const applied = await requests.run(`fragment:${update.target}`, async signal => {
            const response = await fetch(url(update.url), { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
            if (!response.ok) throw new Error(`Fragment unavailable (${response.status})`);
            return response.text();
          }, html => { if (current()) this.rendered(element, { kind: "replace", target: update.target, html }); });
          if (!applied && current()) throw new Error("Fragment request interrupted. Reload the view.");
        } else this.rendered(element, update);
      }).catch(error => { if (current()) throw error; });
      const finished = work.finally(() => {
        if (this.resolving.get(update.target) === finished) this.resolving.delete(update.target);
      });
      this.resolving.set(update.target, finished);
      return finished;
    }
    this.rendered(element, update);
    return Promise.resolve();
  }
  private rendered(element: HTMLElement, update: Exclude<FragmentUpdate, { kind: "ref" }>): void {
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
  /** Inserted shell elements connect themselves: windows, boards, timelines, feeds and seen marks. */
  private apply(element: HTMLElement, update: Exclude<FragmentUpdate, { kind: "ref" }>, follow: boolean): void {
    const reader = element.closest<HTMLElement>("[data-follow]") ?? element;
    const scroll = reader.scrollTop;
    const template = this.doc.createElement("template"); template.innerHTML = update.html;
    if (activeContent(template.content)) { status("Fragment contains unsupported active content."); return; }
    if (update.kind === "append") element.append(template.content);
    else element.replaceChildren(template.content);
    reader.scrollTop = follow ? reader.scrollHeight : scroll;
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

/** One subscription's events as the tab sees them, whatever carries the stream. */
export interface FeedSource {
  addEventListener(type: string, listener: (event: StreamEvent) => void): void;
  onopen: (() => void) | null;
  onerror: (() => void) | null;
  close(): void;
}
/** Snapshot references may finish after another visibility generation starts. */
export async function presentSnapshot(snapshot: Refresh, receive: (update: FragmentUpdate) => Promise<void>, current: () => boolean, recordPosition: (position: string) => void): Promise<boolean> {
  if (!current()) return false;
  await Promise.all(snapshot.fragments.map(value => receive(readUpdate(value))));
  if (!current()) return false;
  recordPosition(snapshot.position);
  return true;
}

/** Receive immediately so newer full snapshots can supersede older requests,
 * but advance the resume position only after preceding events are accounted for. */
export function bindFeed(source: FeedSource, receive: (update: FragmentUpdate) => Promise<void>, recordPosition: (position: string) => void, showStatus: (message: string) => void = status): { settled: () => Promise<void>; close: () => void; requiresReload: () => boolean } {
  let stopped = false;
  let reloadRequired = false;
  let ordered = Promise.resolve();
  const stop = (message: string) => { reloadRequired = true; stopped = true; source.close(); showStatus(message); };
  source.addEventListener(FRAGMENT_EVENT, event => {
    if (stopped) return;
    try {
      const received = receive(readUpdate(JSON.parse(event.data)));
      // Attach a rejection handler immediately even if an older event is pending.
      const accounted = received.then(() => ({ ok: true as const }), error => ({ ok: false as const, error }));
      ordered = ordered.then(async () => {
        const result = await accounted;
        if (stopped) return;
        if (!result.ok) throw result.error;
        if (event.lastEventId) recordPosition(event.lastEventId);
      }).catch(error => { if (!stopped) stop(`${error instanceof Error ? error.message : String(error)} Reload the view.`); });
    } catch (error) { stop(`${error instanceof Error ? error.message : String(error)} Reload the view.`); }
  });
  source.addEventListener(RELOAD_EVENT, event => {
    let reason = event.data;
    try { const value = JSON.parse(event.data) as { reason?: string }; reason = value.reason ?? reason; } catch { /* Preserve an unstructured server reason. */ }
    stop(`Subscription requires reload: ${reason}. Reload the view.`);
  });
  source.onopen = () => { if (!stopped) showStatus(""); };
  source.onerror = () => { if (!stopped) showStatus("Disconnected · retrying"); };
  return { settled: () => ordered, close: () => { stopped = true; source.close(); }, requiresReload: () => reloadRequired };
}

/** A subscription carried by the feed hub, presented as an event source. */
export class SubscriptionSource implements FeedSource {
  private readonly listeners = new Map<string, (event: StreamEvent) => void>();
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(private readonly leave: () => void) {}
  addEventListener(type: string, listener: (event: StreamEvent) => void): void { this.listeners.set(type, listener); }
  emit(type: string, data: string, lastEventId = ""): void { this.listeners.get(type)?.({ data, lastEventId }); }
  close(): void { this.leave(); }
}
/** Route hub messages to the tab's subscriptions. */
export function deliver(message: HubMessage, sources: Map<string, SubscriptionSource>): void {
  if (message.kind === "fragment") sources.get(message.id)?.emit(FRAGMENT_EVENT, message.data, message.position);
  else if (message.kind === "reload") sources.get(message.id)?.emit(RELOAD_EVENT, JSON.stringify({ reason: message.reason }));
  else for (const source of sources.values()) (message.state === "open" ? source.onopen : source.onerror)?.();
}
const sources = new Map<string, SubscriptionSource>();
let hubPort: { postMessage(message: TabMessage): void } | null = null;
let nextSubscription = 0;
/** The browser's one feed hub, or this tab's own where SharedWorker is unavailable. */
function hub(): { postMessage(message: TabMessage): void } {
  if (hubPort) return hubPort;
  let port: MessagePort;
  try {
    port = new SharedWorker("/assets/feed-worker.js", { name: "web-shell feed" }).port;
  } catch {
    const local = new FeedHub(address => new EventSource(address, { withCredentials: true }) as unknown as StreamLike, browserTimer);
    const channel = new MessageChannel();
    local.connect(channel.port1 as unknown as PortLike);
    port = channel.port2;
  }
  port.onmessage = (event: MessageEvent<HubMessage>) => deliver(event.data, sources);
  hubPort = port;
  return port;
}
interface Following { bound: ReturnType<typeof bindFeed> }
const following = new WeakMap<HTMLElement, Following>();
const feeds = new Set<HTMLElement>();
let visibilityGeneration = 0;
async function follow(element: HTMLElement, refresh: boolean, generation: number): Promise<void> {
  if (following.has(element) || element.closest("shell-preview")) return;
  const { route, reference, refresh: refreshRoute } = element.dataset;
  if (!route || !reference || !element.dataset.since) { status("A live feed has no route, reference or position. Reload the view."); return; }
  const current = () => generation === visibilityGeneration && !document.hidden && element.isConnected && !following.has(element);
  if (refresh && refreshRoute) {
    let snapshot: Refresh | undefined;
    const applied = await requests.run(`refresh:${reference}`, async signal => {
      const response = await fetch(url(refreshRoute), { signal, credentials: "same-origin", headers: { Accept: "application/json" } });
      if (!response.ok) throw new Error(`Refresh unavailable (${response.status})`);
      return await response.json() as Refresh;
    }, value => {
      if (!Array.isArray(value.fragments) || typeof value.position !== "string") throw new Error("Refresh must contain fragments and their read position");
      snapshot = value;
    });
    if (!applied || !snapshot) return;
    const presented = await presentSnapshot(snapshot, update => fragments?.receive(update) ?? Promise.resolve(), current, position => { element.dataset.since = position; });
    if (!presented) return;
  }
  if (!current()) return;
  const id = `s${nextSubscription++}`;
  const source = new SubscriptionSource(() => { if (sources.delete(id)) hub().postMessage({ kind: "unsubscribe", id }); });
  sources.set(id, source);
  const bound = bindFeed(source, update => fragments?.receive(update) ?? Promise.resolve(), position => { element.dataset.since = position; });
  following.set(element, { bound });
  hub().postMessage({ kind: "subscribe", route: url(route).href, id, reference, since: element.dataset.since });
}
/** A feed stopped for reload stays stopped until the page is reloaded. */
function unfollow(element: HTMLElement, removed: boolean): void {
  const state = following.get(element);
  if (!state || (state.bound.requiresReload() && !removed)) return;
  state.bound.close(); following.delete(element);
}
async function visibility(returning: boolean): Promise<void> {
  const generation = ++visibilityGeneration;
  if (document.hidden) { for (const element of feeds) unfollow(element, false); requests.abortAll(); return; }
  await Promise.all([...feeds].map(element => follow(element, returning, generation).catch(report)));
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
/** POST the operation request; the answer is a rendered fragment or nothing. */
async function submit(action: Action, signal?: AbortSignal): Promise<string> {
  const response = await fetch(url(action.route), { method: "POST", signal, credentials: "same-origin", headers: { "Content-Type": "application/json", Accept: "text/html" }, body: action.request });
  if (!response.ok) throw new Error(`${action.label} failed (${response.status}). Editor text is retained.`);
  return await response.text();
}
async function runAction(action: Action): Promise<void> {
  const reason = "reason" in action.availability ? action.availability.reason : action.availability.state;
  if (!["enabled", "partial"].includes(action.availability.state)) { status(`${action.label}: ${reason}`); return; }
  if (!action.write) {
    const address = url(action.route);
    const request = JSON.parse(action.request) as { arguments: Record<string, unknown> };
    for (const [key, value] of Object.entries(request.arguments)) {
      if (value !== null && value !== undefined) address.searchParams.set(key, typeof value === "string" ? value : JSON.stringify(value));
    }
    location.assign(address.href); return;
  }
  await requests.run(`operation:${action.operation}`, signal => submit(action, signal), html => {
    status(`${action.label} completed`);
    // The operation may return a server-rendered fragment; an opaque typed JSON answer
    // is never assembled into a browser view.
    if (html.trimStart().startsWith("<")) {
      const selected = heldRow ?? focusedRow();
      if (selected?.id) void fragments?.receive({ kind: "replace", target: selected.id, html }).catch(report);
    }
    dialog("palette")?.close();
  });
}

/** What decides whether a view is shown to a person. */
export interface DisplayState { visibilityState: string; prerendering: boolean; intersecting: boolean; inPreview: boolean; rendered: boolean }
/** `display-acknowledges-only-when-shown`: a visible, non-prerendered document,
 * the element in the viewport and rendered, and not inside a preview. */
export function displayed(state: DisplayState): boolean {
  return state.visibilityState === "visible" && !state.prerendering && state.intersecting && !state.inPreview && state.rendered;
}
/** Sends one seen-mark write, the first time its element is displayed. */
export class SeenMark {
  private sent = false;
  private sending = false;
  constructor(private readonly send: () => Promise<unknown>, private readonly failed: (error: unknown) => void) {}
  async update(state: DisplayState): Promise<boolean> {
    if (this.sent || this.sending || !displayed(state)) return false;
    this.sending = true;
    try { await this.send(); this.sent = true; return true; } catch (error) { this.failed(error); return false; } finally { this.sending = false; }
  }
}

/** Keys of an entered card come before the shared table. */
function enterCard(card: HTMLElement): void {
  card.setAttribute("data-entered", "");
  card.focus();
  const keys = all<HTMLElement>("[data-card-key]", card).map(button => button.dataset.cardKey).join(" ");
  status(`Card keys: ${keys} · Esc leaves the card`);
}
function leaveCard(card: HTMLElement): void {
  card.removeAttribute("data-entered");
  (card.closest<HTMLElement>("[data-row]") ?? card).focus();
  status("");
}
/** Implemented by `<shell-timeline>`; the key table drives it while it has focus. */
interface TimelineControl { stepShape(direction: 1 | -1, edge?: boolean): void; panBy(fraction: number): void; zoomBy(factor: number): void; openShape(): void }
function timelineOf(element: HTMLElement | null): TimelineControl | null {
  const timeline = element?.closest<HTMLElement>("shell-timeline");
  return timeline && "stepShape" in timeline ? timeline as unknown as TimelineControl : null;
}

let prefix = "";
let prefixExpires = 0;
export function resolveKey(key: string, ctrl: boolean, prefix: string = ""): KeyAction | null {
  return SHARED_KEYS.find(binding => binding.key === key && binding.ctrl === ctrl && (binding.prefix ?? "") === prefix)?.action ?? null;
}
export function keydown(event: KeyboardEvent): void {
  const element = target(event);
  if (event.isComposing || event.altKey || event.metaKey) return;
  const card = element?.closest<HTMLElement>("shell-card") ?? null;
  if (card?.hasAttribute("data-entered")) {
    if (event.key === "Escape") { event.preventDefault(); leaveCard(card); return; }
    if (!event.ctrlKey && event.key.length === 1) {
      const button = all<HTMLButtonElement>("[data-card-key]", card).find(button => button.dataset.cardKey === event.key);
      if (button) { event.preventDefault(); if (!button.disabled) button.click(); return; }
    }
  }
  if (resolveKey(event.key, event.ctrlKey) === "close") {
    const activeDialog = element?.closest<HTMLDialogElement>("dialog"); activeDialog?.close();
    const disclosure = element?.closest<HTMLDetailsElement>("details"); if (disclosure) { disclosure.open = false; one<HTMLElement>("summary", disclosure)?.focus(); }
    for (const row of all<HTMLElement>('[aria-selected="true"]')) row.setAttribute("aria-selected", "false");
    element?.closest<HTMLElement>("shell-editor")?.setAttribute("hidden", "");
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
  // A shared sequence (`g g`, `g n`, `g o`, `y c`) comes before a product's navigation letter.
  const sequence = wasPrefix ? resolveKey(event.key, event.ctrlKey, wasPrefix) : null;
  const keyAction = sequence ?? directAction;
  if (resolveKey(wasPrefix, false) === "go_prefix" && sequence === null) {
    const link = all<HTMLAnchorElement>("[data-go]").find(link => link.dataset.go === event.key);
    if (link) { event.preventDefault(); link.click(); return; }
  }
  const timeline = timelineOf(element);
  const palette = element?.closest<HTMLDialogElement>("#palette");
  const candidates = palette ? all<HTMLElement>("[data-action], [data-palette-navigation]", palette).filter(item => !item.closest("[hidden]") && !item.hasAttribute("disabled")) : rows();
  const index = candidates.indexOf((palette ? document.activeElement : focusedRow()) as HTMLElement);
  switch (keyAction) {
    case "focus_query": event.preventDefault(); one<HTMLInputElement>("[data-query]")?.focus(); break;
    case "palette": event.preventDefault(); openDialog("palette"); break;
    case "help": { event.preventDefault(); const help = one<HTMLDetailsElement>("#key-help"); if (help) help.open = !help.open; break; }
    case "go_prefix": prefix = event.key; prefixExpires = Date.now() + 1000; break;
    case "go_notices": {
      event.preventDefault();
      const notices = one<HTMLAnchorElement>("[data-notices]");
      if (notices) notices.click(); else status("This view has no notices list.");
      break;
    }
    case "go_counterpart": {
      event.preventDefault();
      const counterpart = one<HTMLElement>("[data-counterpart]");
      if (counterpart?.getAttribute("href")) counterpart.click();
      else status(counterpart?.dataset.reason ?? "No other product is configured.");
      break;
    }
    case "next": case "previous": {
      event.preventDefault();
      const direction = keyAction === "next" ? 1 : -1;
      if (timeline) timeline.stepShape(direction);
      else focusRow(direction > 0 ? index + 1 : index < 0 ? candidates.length - 1 : index - 1, candidates);
      break;
    }
    case "first": event.preventDefault(); if (timeline) timeline.stepShape(-1, true); else focusRow(0, candidates); break;
    case "last": event.preventDefault(); if (timeline) timeline.stepShape(1, true); else focusRow(candidates.length - 1, candidates); break;
    case "pan_earlier": case "pan_later": if (timeline) { event.preventDefault(); timeline.panBy(keyAction === "pan_later" ? .25 : -.25); } break;
    case "zoom_in": case "zoom_out": if (timeline) { event.preventDefault(); timeline.zoomBy(keyAction === "zoom_in" ? 2 : .5); } break;
    case "copy_command": event.preventDefault(); copyCommand(); break;
    case "open": {
      if (timeline) { event.preventDefault(); timeline.openShape(); break; }
      const row = focusedRow();
      const closedCard = card && !card.hasAttribute("data-entered") && element === card ? card : row && element === row ? one<HTMLElement>("shell-card", row) : null;
      if (closedCard) { event.preventDefault(); enterCard(closedCard); }
      else if (row && element === row) { event.preventDefault(); openRow(); }
      break;
    }
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

export function enhanceWindow(list: HTMLElement): void {
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
      if (activeContent(template.content)) throw new Error("Window contains unsupported active content");
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

export function pruneWindow(list: HTMLElement, route: string, preferred: "before" | "after"): void {
  const box = list.getBoundingClientRect();
  const candidates = rows(list).map(row => {
    const rect = row.getBoundingClientRect();
    return { key: row, visible: rect.bottom > box.top && rect.top < box.bottom, protected: protectedTarget(row) };
  });
  const anchor = candidates.find(row => row.visible)?.key;
  const top = anchor?.getBoundingClientRect().top;
  const maximum = Math.max(1, Number(list.dataset.size) || 200);
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

/** Drag scroll and pinch or Control-wheel zoom over a board's domain range. */
export function boardPointers(element: HTMLElement): void {
  const positions = new Map<number, { x: number; y: number }>();
  let start = 0, end = 1;
  const distance = () => {
    const [first, second] = [...positions.values()];
    return first && second ? Math.hypot(first.x - second.x, first.y - second.y) : undefined;
  };
  const range = () => { start = Number(element.dataset.start ?? 0); end = Number(element.dataset.end ?? 1); };
  const publish = (phase: "preview" | "commit", deltaY = 0) => {
    element.dispatchEvent(new CustomEvent("shell:pointer", { bubbles: true, detail: { kind: "board", start, end, deltaY, phase } }));
    if (phase === "commit" && element.dataset.route && element.id) {
      const address = url(element.dataset.route); address.searchParams.set("start", String(start)); address.searchParams.set("end", String(end));
      void fragments?.receive({ kind: "ref", target: element.id, url: address.href }).catch(report);
    }
  };
  element.addEventListener("pointerdown", event => {
    if ((event.target as HTMLElement).closest("a,button,input,textarea,select,[contenteditable], [data-row]")) return;
    if (positions.size === 0) range(); positions.set(event.pointerId, { x: event.clientX, y: event.clientY });
    element.setPointerCapture(event.pointerId);
  });
  element.addEventListener("pointermove", event => {
    const previous = positions.get(event.pointerId); if (!previous) return;
    const previousDistance = distance();
    positions.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const nextDistance = distance();
    const scale = nextDistance && previousDistance ? nextDistance / previousDistance : 1;
    const deltaX = event.clientX - previous.x;
    const deltaY = event.clientY - previous.y;
    const box = element.getBoundingClientRect();
    [start, end] = pointerRange(start, end, box.width, deltaX, scale, (event.clientX - box.left) / Math.max(1, box.width));
    element.scrollLeft -= deltaX; element.scrollTop -= deltaY;
    publish("preview", deltaY);
  });
  const finish = (event: PointerEvent) => {
    if (!positions.has(event.pointerId)) return;
    positions.delete(event.pointerId);
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

const LABEL_WIDTH = 150;
const AXIS_HEIGHT = 20;
/** Lane labels, the time axis, then every shape; the focused shape and the
 * selection are drawn last. Meaning never rests on colour alone: agency is
 * drawn as fill (operator), outline (agent) or hatching (unknown). */
function drawTimeline(canvas: HTMLCanvasElement, drawn: Layout, window: Window, timeZone: string, focused: number | null, selection: Window | null, colours: Record<string, string>): void {
  const context = canvas.getContext("2d");
  if (!context) return;
  const scale = devicePixelRatio || 1;
  const width = canvas.clientWidth;
  const height = AXIS_HEIGHT + drawn.height;
  canvas.width = Math.round(width * scale); canvas.height = Math.round(height * scale);
  canvas.style.height = `${height}px`;
  context.setTransform(scale, 0, 0, scale, 0, 0);
  context.clearRect(0, 0, width, height);
  context.font = "12px " + (colours.font ?? "sans-serif");
  context.textBaseline = "middle";
  const plot = width - LABEL_WIDTH;
  context.fillStyle = colours.muted ?? "#666";
  for (const tick of ticks(window, plot, timeZone)) {
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
  context.beginPath(); context.rect(LABEL_WIDTH, AXIS_HEIGHT, plot, drawn.height); context.clip();
  const largest = Math.max(1, ...drawn.shapes.map(shape => shape.series.shape.render === "count" ? shape.series.shape.count : 1));
  const ink = colours.ink ?? "#000";
  for (const shape of drawn.shapes) drawShape(context, shape, ink, colours.accent ?? ink, largest);
  const focus = drawn.shapes.find(shape => shape.index === focused);
  if (focus) {
    context.strokeStyle = colours.accent ?? ink; context.lineWidth = 2;
    context.strokeRect(LABEL_WIDTH + focus.x0 - 2, AXIS_HEIGHT + focus.top - 2, focus.x1 - focus.x0 + 4, focus.bottom - focus.top + 4);
  }
  if (selection) {
    const left = (selection.start - window.start) / (window.end - window.start) * plot;
    const right = (selection.end - window.start) / (window.end - window.start) * plot;
    context.globalAlpha = .18; context.fillStyle = colours.accent ?? ink;
    context.fillRect(LABEL_WIDTH + Math.min(left, right), AXIS_HEIGHT, Math.abs(right - left), drawn.height);
    context.globalAlpha = 1;
  }
  context.restore();
}
function drawShape(context: CanvasRenderingContext2D, shape: DrawnShape, ink: string, accent: string, largest: number): void {
  const x = LABEL_WIDTH + shape.x0, y = AXIS_HEIGHT + shape.top, width = Math.max(1, shape.x1 - shape.x0), height = shape.bottom - shape.top;
  const row = shape.series;
  if (row.shape.render === "count") {
    context.globalAlpha = .25 + .75 * Math.log1p(row.shape.count) / Math.log1p(largest);
    context.fillStyle = ink; context.fillRect(x, y, width, height); context.globalAlpha = 1;
    const label = String(row.shape.count);
    if (context.measureText(label).width + 4 < width) { context.fillStyle = accent; context.fillText(label, x + 2, y + height / 2); }
    return;
  }
  const agency = row.shape.agency ?? "unknown";
  context.fillStyle = ink; context.strokeStyle = ink; context.lineWidth = 1;
  if (row.shape.render === "point") {
    const centre = x + width / 2;
    context.beginPath(); context.moveTo(centre, y); context.lineTo(centre + height / 3, y + height / 2); context.lineTo(centre, y + height); context.lineTo(centre - height / 3, y + height / 2); context.closePath();
    if (agency === "agent") context.stroke(); else context.fill();
    return;
  }
  if (agency === "operator") context.fillRect(x, y, width, height);
  else if (agency === "agent") context.strokeRect(x + .5, y + .5, width - 1, height - 1);
  else {
    context.strokeRect(x + .5, y + .5, width - 1, height - 1);
    context.save(); context.beginPath(); context.rect(x, y, width, height); context.clip();
    for (let offset = -height; offset < width; offset += 5) { context.beginPath(); context.moveTo(x + offset, y + height); context.lineTo(x + offset + height, y); context.stroke(); }
    context.restore();
  }
  const label = row.shape.label;
  if (label && context.measureText(label).width + 6 < width) {
    context.fillStyle = agency === "operator" ? accent : ink;
    context.fillText(label, x + 3, y + height / 2);
  }
}

/** A drag selects a window; Control-wheel or a pinch zooms about the pointer;
 * horizontal or Shift-wheel pans. Keys come from the shared table. */
function timelineElement(): CustomElementConstructor {
  return class ShellTimeline extends HTMLElement implements TimelineControl {
    private canvas: HTMLCanvasElement | null = null;
    private description: HTMLElement | null = null;
    private series: Series | null = null;
    private drawn: Layout | null = null;
    private focused: number | null = null;
    private selection: Window | null = null;
    private dragFrom: number | null = null;
    private readonly touches = new Map<number, number>();
    private fetchLater: number | undefined;
    private resize: ResizeObserver | null = null;
    private get window(): Window { return { start: Number(this.dataset.start), end: Number(this.dataset.end) }; }
    connectedCallback(): void {
      if (this.canvas) return;
      const canvas = document.createElement("canvas"); canvas.setAttribute("aria-hidden", "true");
      const description = document.createElement("p"); description.className = "timeline-focus"; description.setAttribute("role", "status"); description.setAttribute("aria-live", "polite");
      this.prepend(canvas); canvas.after(description);
      this.canvas = canvas; this.description = description;
      canvas.addEventListener("pointerdown", event => this.pointerDown(event));
      canvas.addEventListener("pointermove", event => this.pointerMove(event));
      canvas.addEventListener("pointerup", event => this.pointerUp(event));
      canvas.addEventListener("pointercancel", event => { this.touches.delete(event.pointerId); this.dragFrom = null; });
      canvas.addEventListener("dblclick", () => this.openShape());
      canvas.addEventListener("wheel", event => this.wheel(event), { passive: false });
      this.resize = new ResizeObserver(() => this.load());
      this.resize.observe(canvas);
    }
    disconnectedCallback(): void { this.resize?.disconnect(); this.resize = null; this.canvas?.remove(); this.description?.remove(); this.canvas = null; requests.cancel(`timeline:${this.id}`); }
    private plotWidth(): number { return Math.max(1, (this.canvas?.clientWidth ?? 0) - LABEL_WIDTH); }
    private load(): void {
      const route = this.dataset.series; if (!route || !this.canvas) return;
      const address = url(route), window = this.window;
      address.searchParams.set("start", String(window.start)); address.searchParams.set("end", String(window.end)); address.searchParams.set("width", String(Math.round(this.plotWidth())));
      void requests.run(`timeline:${this.id}`, async signal => {
        const response = await fetch(address, { signal, credentials: "same-origin", headers: { Accept: "application/json" } });
        if (!response.ok) throw new Error(`Timeline unavailable (${response.status})`);
        return await response.json() as Series;
      }, series => { this.series = series; this.setAttribute("data-drawn", ""); this.draw(); }).catch(report);
    }
    private draw(): void {
      if (!this.canvas || !this.series) return;
      this.drawn = layout(this.series, this.window, this.plotWidth());
      const style = getComputedStyle(this);
      const colour = (name: string) => style.getPropertyValue(name).trim();
      drawTimeline(this.canvas, this.drawn, this.window, this.dataset.timeZone ?? "UTC", this.focused, this.selection,
        { ink: colour("--ink"), muted: colour("--muted-ink"), line: colour("--line"), accent: colour("--accent"), font: colour("--font-data") });
    }
    /** Draw the current series over the new window at once; fetch its own series shortly after. */
    private show(window: Window): void {
      this.dataset.start = String(window.start); this.dataset.end = String(window.end);
      this.draw();
      this.dispatchEvent(new CustomEvent("shell:window", { bubbles: true, detail: window }));
      clearTimeout(this.fetchLater);
      this.fetchLater = setTimeout(() => this.load(), 150);
    }
    private describe(shape: DrawnShape | null): void {
      if (!this.description) return;
      if (!shape) { this.description.textContent = ""; return; }
      const row = shape.series, format = new Intl.DateTimeFormat("en", { timeZone: this.dataset.timeZone ?? "UTC", dateStyle: "medium", timeStyle: "short" });
      const what = row.shape.render === "count" ? `${row.shape.count} rows · Enter zooms in` : row.shape.label ?? row.shape.reference;
      this.description.textContent = `${this.drawn?.rows[shape.row]?.label ?? row.lane} · ${what} · ${format.format(row.start)}`;
    }
    stepShape(direction: 1 | -1, edge = false): void {
      if (!this.drawn) return;
      const shape = step(this.drawn, edge ? null : this.focused, direction);
      this.focused = shape?.index ?? null; this.draw(); this.describe(shape);
    }
    panBy(fraction: number): void { this.show(pan(this.window, fraction)); }
    zoomBy(factor: number, fraction = .5): void { this.show(zoom(this.window, factor, fraction)); }
    openShape(): void {
      const row = this.focused === null ? null : this.series?.rows[this.focused];
      if (!row) return;
      const inside = countWindow(row);
      if (inside) { this.focused = null; this.show(inside); return; }
      if (row.shape.render !== "count") location.assign(url(row.shape.href).href);
    }
    private fraction(event: { clientX: number }): number {
      const box = this.canvas?.getBoundingClientRect();
      return box ? (event.clientX - box.left - LABEL_WIDTH) / this.plotWidth() : .5;
    }
    private pointerDown(event: PointerEvent): void {
      this.canvas?.setPointerCapture(event.pointerId);
      this.touches.set(event.pointerId, event.clientX);
      this.dragFrom = this.touches.size === 1 ? timeOf(this.fraction(event) * this.plotWidth(), this.window, this.plotWidth()) : null;
    }
    private pointerMove(event: PointerEvent): void {
      const previous = this.touches.get(event.pointerId); if (previous === undefined) return;
      if (this.touches.size === 2) {
        const [first, second] = [...this.touches.values()];
        this.touches.set(event.pointerId, event.clientX);
        const [nextFirst, nextSecond] = [...this.touches.values()];
        if (first === undefined || second === undefined || nextFirst === undefined || nextSecond === undefined) return;
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
    private pointerUp(event: PointerEvent): void {
      this.touches.delete(event.pointerId);
      const selection = this.selection, from = this.dragFrom;
      this.dragFrom = null;
      if (from === null) return;
      const pixels = selection ? (selection.end - selection.start) / (this.window.end - this.window.start) * this.plotWidth() : 0;
      if (!selection || pixels < 3) {
        this.selection = null;
        const box = this.canvas?.getBoundingClientRect();
        const shape = this.drawn && box ? shapeAt(this.drawn, event.clientX - box.left - LABEL_WIDTH, event.clientY - box.top - AXIS_HEIGHT) : null;
        this.focused = shape?.index ?? null; this.draw(); this.describe(shape); this.focus();
        return;
      }
      const window = { start: Math.round(selection.start), end: Math.round(selection.end) };
      this.dispatchEvent(new CustomEvent("shell:select", { bubbles: true, detail: window }));
      const rowsTarget = this.dataset.rows, rowsRoute = this.dataset.rowsRoute;
      if (rowsTarget && rowsRoute) {
        const address = url(rowsRoute); address.searchParams.set("start", String(window.start)); address.searchParams.set("end", String(window.end));
        void fragments?.receive({ kind: "ref", target: rowsTarget, url: address.href }).catch(report);
      }
    }
    private wheel(event: WheelEvent): void {
      if (event.ctrlKey) { event.preventDefault(); this.zoomBy(Math.exp(Math.max(-4, Math.min(4, -event.deltaY / 500))), this.fraction(event)); return; }
      const sideways = event.shiftKey ? event.deltaY : Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : 0;
      if (sideways) { event.preventDefault(); this.panBy(sideways / this.plotWidth()); }
    }
  };
}

/** Previews fetch a fragment of the same view and never acknowledge a seen mark
 * or follow a feed. */
function previewElement(): CustomElementConstructor {
  return class ShellPreview extends HTMLElement {
    private timer: number | undefined;
    private panel: HTMLElement | null = null;
    connectedCallback(): void {
      const show = () => { clearTimeout(this.timer); this.timer = setTimeout(() => void this.show().catch(report), 350); };
      const hide = () => { clearTimeout(this.timer); this.timer = setTimeout(() => this.hide(), 150); };
      this.addEventListener("pointerenter", show); this.addEventListener("focusin", show);
      this.addEventListener("pointerleave", hide); this.addEventListener("focusout", hide);
    }
    disconnectedCallback(): void { clearTimeout(this.timer); this.hide(); }
    private async show(): Promise<void> {
      const route = this.dataset.route; if (!route) return;
      await requests.run(`preview:${route}`, async signal => {
        const response = await fetch(url(route), { signal, credentials: "same-origin", headers: { Accept: "text/html" } });
        if (!response.ok) throw new Error(`Preview unavailable (${response.status})`);
        return await response.text();
      }, html => {
        const template = document.createElement("template"); template.innerHTML = html;
        if (activeContent(template.content)) throw new Error("Preview contains unsupported active content");
        const panel = this.panel ?? document.createElement("div");
        panel.className = "preview-panel"; panel.setAttribute("popover", "manual");
        panel.replaceChildren(template.content);
        if (!this.panel) { this.append(panel); this.panel = panel; }
        const box = this.getBoundingClientRect();
        panel.style.left = `${Math.max(8, Math.min(box.left, innerWidth - 400))}px`; panel.style.top = `${box.bottom + 4}px`;
        panel.showPopover();
      });
    }
    private hide(): void { if (this.panel?.matches(":popover-open")) this.panel.hidePopover(); }
  };
}

function defineElements(): void {
  const define = (name: string, constructor: CustomElementConstructor) => { if (!customElements.get(name)) customElements.define(name, constructor); };
  define("shell-window", class extends HTMLElement { private started = false; connectedCallback(): void { if (!this.started) { this.started = true; enhanceWindow(this); } } });
  define("shell-board", class extends HTMLElement { private started = false; connectedCallback(): void { if (!this.started) { this.started = true; boardPointers(this); } } });
  define("shell-copy", class extends HTMLElement {
    private started = false;
    connectedCallback(): void {
      if (this.started) return; this.started = true;
      one<HTMLButtonElement>("[data-copy-menu]", this)?.addEventListener("click", event => { event.preventDefault(); try { copyMenu(this); } catch (error) { report(error); } });
    }
  });
  define("shell-feed", class extends HTMLElement {
    connectedCallback(): void { feeds.add(this); if (!document.hidden) void follow(this, false, visibilityGeneration).catch(report); }
    disconnectedCallback(): void { feeds.delete(this); unfollow(this, true); }
  });
  define("shell-card", class extends HTMLElement {
    private started = false;
    connectedCallback(): void {
      if (!this.hasAttribute("tabindex")) this.tabIndex = -1;
      if (this.started) return; this.started = true;
      // Leaving the card by any route leaves its key layer.
      this.addEventListener("focusout", event => { if (!(event.relatedTarget instanceof Node && this.contains(event.relatedTarget))) this.removeAttribute("data-entered"); });
    }
    disconnectedCallback(): void { this.removeAttribute("data-entered"); }
  });
  define("shell-editor", class extends HTMLElement {
    private started = false;
    connectedCallback(): void {
      if (this.started) return; this.started = true;
      // Ctrl-Enter sends from any editor or composer.
      this.addEventListener("keydown", event => {
        if (event.key !== "Enter" || !event.ctrlKey) return;
        const form = (event.target as HTMLElement).closest("form") ?? one<HTMLFormElement>("form", this);
        if (form) { event.preventDefault(); form.requestSubmit(); }
      });
    }
  });
  define("shell-seen", class extends HTMLElement {
    private mark: SeenMark | null = null;
    private intersecting = false;
    private observer: IntersectionObserver | null = null;
    private readonly check = () => { void this.mark?.update(this.state()); };
    private state(): DisplayState {
      return { visibilityState: document.visibilityState, prerendering: (document as Document & { prerendering?: boolean }).prerendering === true,
        intersecting: this.intersecting, inPreview: Boolean(this.closest("shell-preview")), rendered: this.checkVisibility?.() ?? true };
    }
    connectedCallback(): void {
      const action = JSON.parse(this.dataset.action ?? "null") as Action | null;
      if (!action) return;
      const fallback = one<HTMLElement>("[data-action-item]", this);
      if (fallback) fallback.hidden = true;
      this.mark = new SeenMark(() => submit(action), error => { if (fallback) fallback.hidden = false; report(error); });
      this.observer = new IntersectionObserver(entries => { this.intersecting = entries.some(entry => entry.isIntersecting); this.check(); });
      this.observer.observe(this);
      document.addEventListener("visibilitychange", this.check);
      document.addEventListener("prerenderingchange", this.check);
    }
    disconnectedCallback(): void {
      this.observer?.disconnect(); this.observer = null; this.mark = null;
      document.removeEventListener("visibilitychange", this.check);
      document.removeEventListener("prerenderingchange", this.check);
    }
  });
  define("shell-timeline", timelineElement());
  define("shell-preview", previewElement());
}

function boot(): void {
  document.addEventListener("focusin", event => { const row = target(event)?.closest<HTMLElement>("[data-row]"); if (row) lastFocusedRow = row; });
  document.addEventListener("keydown", keydown);
  document.addEventListener("click", event => {
    const element = target(event);
    if (element?.closest("[data-open-palette]")) openDialog("palette");
    if (element?.closest("[data-open-help]")) { const help = one<HTMLDetailsElement>("#key-help"); if (help) help.open = !help.open; }
    if (element?.closest("[data-close-dialog]")) element.closest<HTMLDialogElement>("dialog")?.close();
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
  document.addEventListener("visibilitychange", () => { void visibility(true).catch(report); });
  window.addEventListener("pagehide", () => { for (const element of feeds) unfollow(element, false); hubPort?.postMessage({ kind: "leave" }); requests.abortAll(); });
  window.addEventListener("pageshow", event => { if (event.persisted) void visibility(true).catch(report); });
  defineElements();
}
if (typeof document !== "undefined") {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot, { once: true }); else boot();
}
