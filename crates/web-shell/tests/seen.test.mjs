import test from "node:test";
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
const { displayed, SeenMark } = await import(pathToFileURL(process.env.SHELL_MODULE));

const shown = { visibilityState: "visible", prerendering: false, intersecting: true, inPreview: false, rendered: true };
const notShown = [
  { prerendering: true },
  { visibilityState: "hidden" },
  { intersecting: false },
  { inPreview: true },
  { rendered: false },
];

test("display-acknowledges-only-when-shown: prerendered, hidden, offscreen, unrendered and previewed views write nothing", async () => {
  assert.equal(displayed(shown), true);
  let writes = 0;
  const mark = new SeenMark(async () => { writes += 1; }, () => {});
  for (const change of notShown) {
    assert.equal(displayed({ ...shown, ...change }), false, JSON.stringify(change));
    assert.equal(await mark.update({ ...shown, ...change }), false);
  }
  assert.equal(writes, 0);
  // Prerendered, then shown: written once, at the showing.
  assert.equal(await mark.update(shown), true);
  assert.equal(await mark.update(shown), false);
  assert.equal(writes, 1);
});

test("checks racing one showing send one write", async () => {
  let writes = 0, finish;
  const mark = new SeenMark(() => new Promise(resolve => { writes += 1; finish = resolve; }), () => {});
  const first = mark.update(shown), second = mark.update(shown);
  finish();
  assert.deepEqual(await Promise.all([first, second]), [true, false]);
  assert.equal(writes, 1);
});

test("a failed seen write is reported and sent again at the next showing", async () => {
  let attempts = 0;
  const failures = [];
  const mark = new SeenMark(async () => { attempts += 1; if (attempts === 1) throw new Error("daemon offline"); }, error => failures.push(error.message));
  assert.equal(await mark.update(shown), false);
  assert.deepEqual(failures, ["daemon offline"]);
  assert.equal(await mark.update(shown), true);
  assert.equal(attempts, 2);
});
