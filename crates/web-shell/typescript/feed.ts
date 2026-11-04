/** The feed hub: one event stream per product route for every tab of a browser
 * profile. It runs in a SharedWorker; where SharedWorker is unavailable, each
 * tab runs its own hub, which then holds that tab's one stream. */
import { FRAGMENT_EVENT, RELOAD_EVENT, SUBSCRIPTIONS_PARAMETER } from "./types";
import type { FeedEvent, FeedReload, Subscription } from "./types";

/** What a tab tells the hub. Tab subscription IDs are unique within the tab. */
export type TabMessage =
  | { kind: "subscribe"; route: string; id: string; reference: string; since: string }
  | { kind: "unsubscribe"; id: string }
  | { kind: "leave" };
/** What the hub tells a tab. `data` is the fragment update as JSON text. */
export type HubMessage =
  | { kind: "fragment"; id: string; position: string; data: string }
  | { kind: "reload"; id: string; reason: string }
  | { kind: "connection"; state: "open" | "retrying" };

export interface PortLike { postMessage(message: HubMessage): void; onmessage: ((event: { data: TabMessage }) => void) | null }
export interface StreamEvent { data: string; lastEventId: string }
export interface StreamLike {
  addEventListener(type: string, listener: (event: StreamEvent) => void): void;
  onopen: (() => void) | null;
  onerror: (() => void) | null;
  close(): void;
}
/** Run `callback` after `delay` milliseconds; the result cancels it. */
export type Timer = (callback: () => void, delay: number) => () => void;

interface Entry { port: PortLike; tabId: string; route: string; reference: string; since: string }
interface Connection { stream: StreamLike | null; pending: (() => void) | null; failures: number }

/** Subscription changes within this many milliseconds share one reconnection. */
export const COALESCE_MILLISECONDS = 20;
const RETRY_LIMIT_MILLISECONDS = 30_000;

export class FeedHub {
  private readonly entries = new Map<string, Entry>();
  private readonly connections = new Map<string, Connection>();
  private nextPort = 0;
  constructor(private readonly open: (url: string) => StreamLike, private readonly timer: Timer) {}

  connect(port: PortLike): void {
    const number = this.nextPort++;
    port.onmessage = event => this.receive(port, number, event.data);
  }
  /** Open event streams; at most one per route. */
  openStreams(): number { return [...this.connections.values()].filter(connection => connection.stream).length; }

  private receive(port: PortLike, number: number, message: TabMessage): void {
    if (message.kind === "subscribe") {
      this.entries.set(`${number}.${message.id}`, { port, tabId: message.id, route: message.route, reference: message.reference, since: message.since });
      this.changed(message.route);
    } else if (message.kind === "unsubscribe") {
      const key = `${number}.${message.id}`;
      const entry = this.entries.get(key);
      if (entry) { this.entries.delete(key); this.changed(entry.route); }
    } else {
      for (const [key, entry] of [...this.entries]) {
        if (entry.port === port) { this.entries.delete(key); this.changed(entry.route); }
      }
    }
  }
  private connection(route: string): Connection {
    let connection = this.connections.get(route);
    if (!connection) { connection = { stream: null, pending: null, failures: 0 }; this.connections.set(route, connection); }
    return connection;
  }
  private changed(route: string, delay = COALESCE_MILLISECONDS): void {
    const connection = this.connection(route);
    if (connection.pending) return;
    connection.pending = this.timer(() => { connection.pending = null; this.reopen(route); }, delay);
  }
  private routeEntries(route: string): Array<[string, Entry]> { return [...this.entries].filter(([, entry]) => entry.route === route); }
  /** Each subscription resumes from the last position delivered to its tab, so
   * a reconnection neither repeats nor skips an event. */
  private reopen(route: string): void {
    const connection = this.connection(route);
    connection.stream?.close(); connection.stream = null;
    const entries = this.routeEntries(route);
    if (!entries.length) { this.connections.delete(route); return; }
    const subscriptions: Subscription[] = entries.map(([id, entry]) => ({ id, reference: entry.reference, since: entry.since }));
    const address = `${route}${route.includes("?") ? "&" : "?"}${SUBSCRIPTIONS_PARAMETER}=${encodeURIComponent(JSON.stringify(subscriptions))}`;
    const stream = this.open(address);
    connection.stream = stream;
    const current = () => connection.stream === stream;
    stream.addEventListener(FRAGMENT_EVENT, event => {
      if (!current()) return;
      let value: FeedEvent;
      try { value = JSON.parse(event.data) as FeedEvent; } catch { this.reload(route, null, "the stream sent an unreadable event"); return; }
      const entry = this.entries.get(value.subscription);
      if (!entry || entry.route !== route) return;
      if (event.lastEventId) entry.since = event.lastEventId;
      entry.port.postMessage({ kind: "fragment", id: entry.tabId, position: event.lastEventId, data: JSON.stringify(value.update) });
    });
    stream.addEventListener(RELOAD_EVENT, event => {
      if (!current()) return;
      let value: FeedReload;
      try { value = JSON.parse(event.data) as FeedReload; } catch { value = { reason: event.data }; }
      this.reload(route, value.subscription ?? null, value.reason);
    });
    stream.onopen = () => {
      if (!current()) return;
      connection.failures = 0;
      this.broadcast(route, { kind: "connection", state: "open" });
    };
    stream.onerror = () => {
      if (!current()) return;
      // Reopen with the delivered positions instead of the browser's reconnect,
      // which would resend the positions the stream was first opened with.
      stream.close(); connection.stream = null;
      this.broadcast(route, { kind: "connection", state: "retrying" });
      connection.failures += 1;
      connection.pending?.(); connection.pending = null;
      this.changed(route, Math.min(RETRY_LIMIT_MILLISECONDS, 1000 * 2 ** (connection.failures - 1)));
    };
  }
  private reload(route: string, subscription: string | null, reason: string): void {
    for (const [key, entry] of this.routeEntries(route)) {
      if (subscription !== null && key !== subscription) continue;
      entry.port.postMessage({ kind: "reload", id: entry.tabId, reason });
      this.entries.delete(key);
    }
    this.changed(route);
  }
  private broadcast(route: string, message: HubMessage): void {
    const ports = new Set(this.routeEntries(route).map(([, entry]) => entry.port));
    for (const port of ports) port.postMessage(message);
  }
}

export function browserTimer(callback: () => void, delay: number): () => void {
  const handle = setTimeout(callback, delay);
  return () => clearTimeout(handle);
}
