/** SharedWorker entry: every tab of the browser profile connects to one hub. */
import { FeedHub, browserTimer } from "./feed";
import type { PortLike, StreamLike } from "./feed";

const hub = new FeedHub(url => new EventSource(url, { withCredentials: true }) as unknown as StreamLike, browserTimer);
const scope = globalThis as unknown as { onconnect: ((event: MessageEvent) => void) | null };
scope.onconnect = event => { const port = event.ports[0]; if (port) hub.connect(port as unknown as PortLike); };
