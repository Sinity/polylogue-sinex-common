import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
const timeline = await import(pathToFileURL(process.env.TIMELINE_MODULE));

// Fitted by Rust (tests/timeline.rs) at year, month, day and hour windows.
const levels = JSON.parse(readFileSync(new URL("timeline-zoom.json", import.meta.url), "utf8"));
const end = row => timeline.rowEnd(row);

test("timeline zoom fixture: every fitted row is drawn inside the plot, at least the minimum wide", () => {
  assert.equal(levels.length, 4);
  for (const series of levels) {
    const drawn = timeline.layout(series, series.window, series.width);
    assert.equal(drawn.shapes.length, series.rows.length, "every row touches the window and is drawn");
    for (const shape of drawn.shapes) {
      assert.ok(shape.x0 >= 0 && shape.x1 <= series.width, JSON.stringify(shape));
      const clipped = shape.x0 === 0 || shape.x1 === series.width;
      if (!clipped) assert.ok(shape.x1 - shape.x0 >= series.minimum_pixels - 1e-9);
      assert.equal(drawn.rows[shape.row].lane, shape.series.lane);
    }
    // One drawn row per lane, or per group of a grouped lane.
    const lanes = new Set(drawn.rows.map(row => row.lane));
    assert.deepEqual([...lanes], series.lanes.map(lane => lane.name));
  }
});

test("timeline zoom fixture: the shape under its own centre is found again", () => {
  for (const series of levels) {
    const drawn = timeline.layout(series, series.window, series.width);
    for (const shape of drawn.shapes) {
      const x = (shape.x0 + shape.x1) / 2, y = (shape.top + shape.bottom) / 2;
      const found = timeline.shapeAt(drawn, x, y);
      assert.ok(found && found.x0 <= x && x <= found.x1 && found.row === shape.row);
    }
    assert.equal(timeline.shapeAt(drawn, -5, 5), null);
  }
});

test("timeline zoom fixture: opening a count zooms into its span, and the finer level draws its rows", () => {
  const [year, month] = levels;
  const count = year.rows.find(row => row.shape.render === "count" && row.start >= month.window.start && end(row) <= month.window.end);
  assert.ok(count, "the year level bins October");
  const inside = timeline.countWindow(count);
  assert.deepEqual(inside, { start: count.start, end: count.shape.end });
  // The count stands for the rows that start in its bin; it may end later, where its last member ends.
  const finer = month.rows.filter(row => row.start >= count.start && row.start < count.start + year.bin && row.lane === count.lane && row.group === count.group);
  const weight = finer.reduce((sum, row) => sum + (row.shape.render === "count" ? row.shape.count : 1), 0);
  assert.equal(weight, count.shape.count, "the month level accounts for every row the year level counted");
  assert.equal(timeline.countWindow(month.rows.find(row => row.shape.render !== "count")), null);
});

test("timeline zoom keeps the time under the pointer, and pan keeps the span", () => {
  const window = levels[1].window;
  for (const fraction of [0, .25, .5, .9]) {
    const before = timeline.timeOf(fraction * 480, window, 480);
    const zoomed = timeline.zoom(window, 2, fraction);
    assert.ok(Math.abs(timeline.timeOf(fraction * 480, zoomed, 480) - before) <= 1);
    assert.ok(Math.abs((zoomed.end - zoomed.start) * 2 - (window.end - window.start)) <= 2);
  }
  const panned = timeline.pan(window, -.25);
  assert.equal(panned.end - panned.start, window.end - window.start);
  assert.ok(panned.start < window.start);
});

test("while a zoom waits for its series, the coarser series is drawn at the new scale", () => {
  const [year, month] = levels;
  const drawn = timeline.layout(year, month.window, month.width);
  for (const shape of drawn.shapes) {
    const row = shape.series;
    const x0 = timeline.xOf(row.start, month.window, month.width), x1 = timeline.xOf(end(row), month.window, month.width);
    if (x1 - x0 >= year.minimum_pixels && x0 >= 0 && x1 <= month.width) {
      assert.ok(Math.abs(shape.x0 - x0) < 1e-9 && Math.abs(shape.x1 - x1) < 1e-9);
    }
  }
});

test("keyboard order visits every shape once, row by row from left to right", () => {
  const series = levels[2];
  const drawn = timeline.layout(series, series.window, series.width);
  const visited = [];
  for (let shape = timeline.step(drawn, null, 1); shape && !visited.includes(shape.index); shape = timeline.step(drawn, shape.index, 1)) visited.push(shape.index);
  assert.equal(visited.length, drawn.shapes.length);
  const order = visited.map(index => drawn.shapes.find(shape => shape.index === index));
  for (let index = 1; index < order.length; index++) {
    const [previous, next] = [order[index - 1], order[index]];
    assert.ok(previous.row < next.row || (previous.row === next.row && previous.x0 <= next.x0));
  }
  assert.equal(timeline.step(drawn, null, -1).index, visited.at(-1));
});

test("axis ticks are spaced apart and labelled in the series' time zone", () => {
  for (const series of levels) {
    const marks = timeline.ticks(series.window, series.width, series.time_zone);
    assert.ok(marks.length >= 2, JSON.stringify(series.window));
    for (let index = 1; index < marks.length; index++) assert.ok(marks[index].x - marks[index - 1].x >= 60 - 1e-9);
  }
  const hour = levels[3];
  // 14:00 UTC is 16:00 in Warsaw in October.
  assert.ok(timeline.ticks(hour.window, hour.width, "Europe/Warsaw").some(mark => mark.label === "16:00"));
});
