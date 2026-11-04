import test from "node:test";
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
const { FeedHub } = await import(pathToFileURL(process.env.FEED_MODULE));
const { SubscriptionSource, deliver, bindFeed } = await import(pathToFileURL(process.env.SHELL_MODULE));

const ROUTE = "https://example.test/events";
class Stream {
  constructor(url) { this.url = url; this.listeners = new Map(); this.closed = false; this.onopen = null; this.onerror = null; }
  addEventListener(type, listener) { this.listeners.set(type, listener); }
  emit(type, data, lastEventId = "") { this.listeners.get(type)?.({ data, lastEventId }); }
  close() { this.closed = true; }
  subscriptions() { return JSON.parse(new URL(this.url).searchParams.get("subscriptions")); }
}
/** A browser profile: tabs are ports, the event connections are counted. */
function browser() {
  const streams = [], timers = [];
  const hub = new FeedHub(url => { const stream = new Stream(url); streams.push(stream); return stream; },
    (callback, delay) => { const timer = { callback, delay, cancelled: false, ran: false }; timers.push(timer); return () => { timer.cancelled = true; }; });
  const settle = () => { for (let timer; (timer = timers.find(item => !item.cancelled && !item.ran));) { timer.ran = true; timer.callback(); } };
  const tab = () => {
    const port = { received: [], onmessage: null, postMessage(message) { this.received.push(message); }, send(message) { this.onmessage({ data: message }); } };
    hub.connect(port); return port;
  };
  const open = () => streams.filter(stream => !stream.closed);
  return { hub, streams, timers, settle, tab, open };
}
const subscribe = (id, reference, since = "p1") => ({ kind: "subscribe", route: ROUTE, id, reference, since });
const update = html => ({ kind: "append", target: "reader", html });

test("one-connection-per-browser: five tabs hold one event connection carrying every subscription", () => {
  const profile = browser();
  const tabs = Array.from({ length: 5 }, () => profile.tab());
  tabs.forEach((tab, index) => { tab.send(subscribe("s0", `session:${index}`)); tab.send(subscribe("s1", "waiting")); });
  profile.settle();
  assert.equal(profile.streams.length, 1, "page loads coalesce into one connection");
  assert.equal(profile.hub.openStreams(), 1);
  const subscriptions = profile.open()[0].subscriptions();
  assert.equal(subscriptions.length, 10);
  assert.equal(new Set(subscriptions.map(item => item.id)).size, 10, "hub IDs never collide across tabs");
  // A sixth tab joins the same connection; it never opens its own.
  profile.tab().send(subscribe("s0", "page:home")); profile.settle();
  assert.equal(profile.hub.openStreams(), 1);
  assert.equal(profile.open()[0].subscriptions().length, 11);
});

test("events reach only their subscription, and a reconnection resumes from delivered positions", () => {
  const profile = browser(); const [first, second] = [profile.tab(), profile.tab()];
  first.send(subscribe("s0", "session:a")); second.send(subscribe("s0", "session:b")); profile.settle();
  const stream = profile.open()[0];
  const ids = Object.fromEntries(stream.subscriptions().map(item => [item.reference, item.id]));
  stream.emit("fragment", JSON.stringify({ subscription: ids["session:b"], update: update("B") }), "p2");
  assert.deepEqual(first.received, []);
  assert.deepEqual(second.received, [{ kind: "fragment", id: "s0", position: "p2", data: JSON.stringify(update("B")) }]);
  first.send({ kind: "unsubscribe", id: "s0" }); profile.settle();
  assert.equal(stream.closed, true);
  assert.deepEqual(profile.open()[0].subscriptions(), [{ id: ids["session:b"], reference: "session:b", since: "p2" }]);
  // Events of the closed connection that arrive late are not delivered.
  stream.emit("fragment", JSON.stringify({ subscription: ids["session:b"], update: update("late") }), "p3");
  assert.equal(second.received.length, 1);
  second.send({ kind: "leave" }); profile.settle();
  assert.equal(profile.hub.openStreams(), 0);
});

test("a reload for one subscription stops only that subscription", () => {
  const profile = browser(); const [first, second] = [profile.tab(), profile.tab()];
  first.send(subscribe("s0", "session:a")); second.send(subscribe("s0", "session:b")); profile.settle();
  const stream = profile.open()[0];
  const ids = Object.fromEntries(stream.subscriptions().map(item => [item.reference, item.id]));
  stream.emit("reload", JSON.stringify({ subscription: ids["session:a"], reason: "position no longer retained" }));
  assert.deepEqual(first.received, [{ kind: "reload", id: "s0", reason: "position no longer retained" }]);
  assert.deepEqual(second.received, []);
  profile.settle();
  assert.deepEqual(profile.open()[0].subscriptions().map(item => item.reference), ["session:b"]);
  profile.open()[0].emit("reload", JSON.stringify({ reason: "generation changed" }));
  assert.deepEqual(second.received, [{ kind: "reload", id: "s0", reason: "generation changed" }]);
  profile.settle();
  assert.equal(profile.hub.openStreams(), 0);
});

test("a failed connection reopens with delivered positions and growing delays", () => {
  const profile = browser(); const tab = profile.tab();
  tab.send(subscribe("s0", "session:a")); profile.settle();
  let stream = profile.open()[0];
  const id = stream.subscriptions()[0].id;
  stream.emit("fragment", JSON.stringify({ subscription: id, update: update("A") }), "p7");
  stream.onerror();
  assert.equal(stream.closed, true);
  assert.deepEqual(tab.received.at(-1), { kind: "connection", state: "retrying" });
  assert.equal(profile.timers.at(-1).delay, 1000);
  profile.settle(); stream = profile.open()[0];
  assert.equal(stream.subscriptions()[0].since, "p7");
  stream.onerror(); assert.equal(profile.timers.at(-1).delay, 2000);
  profile.settle(); profile.open()[0].onopen();
  assert.deepEqual(tab.received.at(-1), { kind: "connection", state: "open" });
  profile.open()[0].onerror(); assert.equal(profile.timers.at(-1).delay, 1000, "an open connection resets the delay");
});

test("each product route has its own connection", () => {
  const profile = browser(); const tab = profile.tab();
  tab.send(subscribe("s0", "session:a")); tab.send({ ...subscribe("s1", "waiting"), route: "https://example.test/other-events" });
  profile.settle();
  assert.equal(profile.hub.openStreams(), 2);
});

test("a tab's subscriptions present hub messages as event sources; a reload stops only its feed", async () => {
  const sources = new Map(), left = [];
  const first = new SubscriptionSource(() => left.push("first")), second = new SubscriptionSource(() => left.push("second"));
  sources.set("s0", first); sources.set("s1", second);
  const received = [], positions = [], statuses = [];
  const one = bindFeed(first, async value => { received.push(value.html); }, position => positions.push(position), message => statuses.push(message));
  const two = bindFeed(second, async () => {}, () => {}, message => statuses.push(message));
  deliver({ kind: "fragment", id: "s0", position: "p5", data: JSON.stringify(update("A")) }, sources);
  await one.settled();
  assert.deepEqual(received, ["A"]); assert.deepEqual(positions, ["p5"]);
  deliver({ kind: "reload", id: "s1", reason: "cannot follow page:x" }, sources);
  assert.equal(two.requiresReload(), true); assert.equal(one.requiresReload(), false);
  assert.deepEqual(left, ["second"]);
  assert.match(statuses.at(-1), /cannot follow page:x.*Reload the view/);
  deliver({ kind: "connection", state: "retrying" }, sources);
  assert.equal(statuses.at(-1), "Disconnected · retrying");
});
