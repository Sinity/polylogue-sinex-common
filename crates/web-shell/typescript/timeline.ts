/** Timeline geometry: where each series row is drawn for a window and width,
 * which shape is under the pointer, keyboard order, zoom and pan. The drawing
 * itself is in the `<shell-timeline>` element. */
import { pointerRange } from "./core";
import type { Series, SeriesRow, Window } from "./types";

export const ROW_HEIGHT = 22;
const SHAPE_INSET = 4;

export interface DrawnRow { lane: string; group: string | null; label: string; top: number }
export interface DrawnShape { index: number; row: number; x0: number; x1: number; top: number; bottom: number; series: SeriesRow }
export interface Layout { rows: DrawnRow[]; shapes: DrawnShape[]; height: number }

export function xOf(time: number, window: Window, width: number): number {
  return (time - window.start) / (window.end - window.start) * width;
}
export function timeOf(x: number, window: Window, width: number): number {
  return window.start + x / Math.max(1, width) * (window.end - window.start);
}
function rowKey(lane: string, group: string | null): string { return JSON.stringify([lane, group]); }
export function rowEnd(row: SeriesRow): number { return row.shape.render === "point" ? row.start : row.shape.end; }

/** Lay the series out over `window`, which may differ from the window the
 * series was fitted to while a pan or zoom is in progress. Each lane takes one
 * drawn row per group; a shape is never narrower than the minimum. */
export function layout(series: Series, window: Window, width: number): Layout {
  const rows: DrawnRow[] = [];
  const index = new Map<string, number>();
  for (const lane of series.lanes) {
    const groups = lane.group_by === null
      ? [null]
      : [...new Set(series.rows.filter(row => row.lane === lane.name).map(row => row.group ?? ""))].sort();
    for (const group of groups) {
      index.set(rowKey(lane.name, group), rows.length);
      rows.push({ lane: lane.name, group, label: group === null ? lane.label : `${lane.label} · ${group}`, top: rows.length * ROW_HEIGHT });
    }
  }
  const minimum = series.minimum_pixels;
  const shapes: DrawnShape[] = [];
  series.rows.forEach((item, position) => {
    const row = index.get(rowKey(item.lane, item.group));
    if (row === undefined) return;
    let x0 = xOf(item.start, window, width);
    let x1 = xOf(rowEnd(item), window, width);
    if (x1 - x0 < minimum) {
      const centre = item.shape.render === "point" ? x0 : (x0 + x1) / 2;
      x0 = centre - minimum / 2; x1 = centre + minimum / 2;
    }
    if (x1 < 0 || x0 > width) return;
    const top = row * ROW_HEIGHT + SHAPE_INSET;
    shapes.push({ index: position, row, x0: Math.max(0, x0), x1: Math.min(width, x1), top, bottom: top + ROW_HEIGHT - 2 * SHAPE_INSET, series: item });
  });
  return { rows, shapes, height: rows.length * ROW_HEIGHT };
}

/** The topmost shape at a point; shapes drawn later are on top. */
export function shapeAt(drawn: Layout, x: number, y: number): DrawnShape | null {
  for (let index = drawn.shapes.length - 1; index >= 0; index--) {
    const shape = drawn.shapes[index];
    if (shape && x >= shape.x0 && x <= shape.x1 && y >= shape.top && y <= shape.bottom) return shape;
  }
  return null;
}
/** Keyboard order: drawn row by drawn row, left to right. */
export function step(drawn: Layout, current: number | null, direction: 1 | -1): DrawnShape | null {
  const ordered = [...drawn.shapes].sort((left, right) => left.row - right.row || left.x0 - right.x0 || left.index - right.index);
  if (!ordered.length) return null;
  const at = ordered.findIndex(shape => shape.index === current);
  if (at < 0) return direction > 0 ? ordered[0] ?? null : ordered.at(-1) ?? null;
  return ordered[Math.max(0, Math.min(ordered.length - 1, at + direction))] ?? null;
}
/** Zoom by `factor` (above 1 zooms in) keeping the time at `fraction` of the width fixed. */
export function zoom(window: Window, factor: number, fraction: number): Window {
  const [start, end] = pointerRange(window.start, window.end, 1, 0, factor, fraction);
  return { start: Math.round(start), end: Math.max(Math.round(start) + 1, Math.round(end)) };
}
/** Pan by a fraction of the span; negative is earlier. */
export function pan(window: Window, fraction: number): Window {
  const shift = Math.round((window.end - window.start) * fraction);
  return { start: window.start + shift, end: window.end + shift };
}
/** The window a count row stands for: opening it zooms there. */
export function countWindow(row: SeriesRow): Window | null {
  return row.shape.render === "count" ? { start: row.start, end: row.shape.end } : null;
}

const STEPS = [1e3, 5e3, 15e3, 6e4, 3e5, 9e5, 36e5, 108e5, 216e5, 432e5, 864e5, 6048e5, 2592e6, 7776e6, 31536e6];
/** Axis ticks at least `spacing` pixels apart, labelled in the series' time zone. */
export function ticks(window: Window, width: number, timeZone: string, spacing = 60): Array<{ x: number; label: string }> {
  const span = window.end - window.start;
  const size = STEPS.find(candidate => candidate / span * width >= spacing) ?? STEPS.at(-1) ?? span;
  const options: Intl.DateTimeFormatOptions = size >= 864e5
    ? { timeZone, year: size >= 2592e6 ? "numeric" : undefined, month: "short", day: size >= 2592e6 ? undefined : "numeric" }
    : { timeZone, hour: "2-digit", minute: "2-digit", second: size < 6e4 ? "2-digit" : undefined, hourCycle: "h23" };
  const format = new Intl.DateTimeFormat("en", options);
  const result: Array<{ x: number; label: string }> = [];
  for (let time = Math.ceil(window.start / size) * size; time < window.end; time += size) {
    result.push({ x: xOf(time, window, width), label: format.format(time) });
  }
  return result;
}
