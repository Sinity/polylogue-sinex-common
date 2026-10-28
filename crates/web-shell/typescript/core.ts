/** Shared non-DOM state machines, exercised by the browser and Node tests. */
export class LatestRequest {
  private current = new Map<string, { controller: AbortController; generation: number }>();
  private generation = 0;
  async run<T>(key: string, read: (signal: AbortSignal) => Promise<T>, apply: (value: T) => void): Promise<boolean> {
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
  cancel(key: string): void { this.current.get(key)?.controller.abort(); this.current.delete(key); }
  abortAll(): void { for (const request of this.current.values()) request.controller.abort(); this.current.clear(); }
}
export interface ReaderState { following: boolean; atEnd: boolean; protected: boolean }
export function shouldHold(kind: "append" | "replace", state: ReaderState): boolean {
  return state.protected || (kind === "append" && (!state.following || !state.atEnd));
}
/** Subsequence score. Both web and terminal callers can use these vectors as the matcher contract. */
export function matchScore(query: string, label: string): number | null {
  const needle = Array.from(query.toLowerCase());
  const haystack = Array.from(label.toLowerCase());
  let next = 0, score = 0, previous = -2;
  for (const character of needle) {
    let found = -1;
    for (let index = next; index < haystack.length; index++) { if (haystack[index] === character) { found = index; break; } }
    if (found < 0) return null;
    score += found === previous + 1 ? 8 : 1;
    if (found === 0 || /[ \t\n\r\v\f_/-]/u.test(haystack[found - 1] ?? "")) score += 4;
    previous = found; next = found + 1;
  }
  return score - (haystack.length - needle.length) / 1000;
}
export function nearEnd(scrollTop: number, height: number, total: number): boolean { return total - scrollTop - height <= 2; }
export function pointerRange(start: number, end: number, width: number, delta: number, scale: number, fraction: number): [number, number] {
  const span = end - start;
  const nextSpan = span / scale;
  const anchor = start + fraction * span;
  const nextStart = anchor - fraction * nextSpan - delta / Math.max(1, width) * span;
  return [nextStart, nextStart + nextSpan];
}
export function sameOrigin(value: string, base: string): URL {
  const url = new URL(value, base);
  if (url.origin !== new URL(base).origin || !["http:", "https:"].includes(url.protocol)) throw new Error("Operation URL must be on this origin");
  return url;
}
export interface ClipboardPort { writeText(text: string): Promise<void> }
export async function copyText(text: string, clipboard: ClipboardPort | undefined): Promise<{ copied: boolean; characters: number }> {
  try {
    if (!clipboard) return { copied: false, characters: Array.from(text).length };
    await clipboard.writeText(text);
    return { copied: true, characters: Array.from(text).length };
  } catch { return { copied: false, characters: Array.from(text).length }; }
}

/** Remove only contiguous outer edges. A protected/visible boundary stops that edge;
 * rows between surviving anchors are never skipped and remain reachable in order. */
export function windowTrim<T>(rows: Array<{ key: T; visible: boolean; protected: boolean }>, maximum: number, preferred: "before" | "after"): { remove: T[]; before: boolean; after: boolean } {
  const remaining = [...rows];
  const remove: T[] = [];
  const result = { remove, before: false, after: false };
  const edges: Array<"before" | "after"> = [preferred, preferred === "before" ? "after" : "before"];
  for (const edge of edges) {
    while (remaining.length > maximum) {
      const row = edge === "before" ? remaining[0] : remaining.at(-1);
      if (!row || row.visible || row.protected) break;
      remove.push(row.key); result[edge] = true;
      if (edge === "before") remaining.shift(); else remaining.pop();
    }
  }
  return result;
}
