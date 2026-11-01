"use strict";
(() => {
  // typescript/types.ts
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

  // typescript/feed-worker.ts
  var hub = new FeedHub((url) => new EventSource(url, { withCredentials: true }), browserTimer);
  var scope = globalThis;
  scope.onconnect = (event) => {
    const port = event.ports[0];
    if (port) hub.connect(port);
  };
})();
