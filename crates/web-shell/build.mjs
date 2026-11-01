#!/usr/bin/env node
// No npm installation. Use the toolchain supplied by the development shell.
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
const root = dirname(fileURLToPath(import.meta.url));
const scratch = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), "web-shell-"));
function run(program, args, extra = {}) {
  const result = spawnSync(program, args, { cwd: root, stdio: "inherit", ...extra });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${program} failed (${result.status})`);
}
function bundle(entry, format, name) {
  const output = join(scratch, name);
  run(process.env.ESBUILD ?? "esbuild", [entry, "--bundle", `--format=${format}`, "--target=es2022", `--outfile=${output}`]);
  return output;
}
try {
  run(process.env.TSC ?? "tsc", ["--project", join(root, "tsconfig.json")]);
  // The page's shell is a module; the SharedWorker is a classic script.
  // esbuild's output depends only on source and its version; output filename is absent.
  const artifacts = { "assets/shell.js": bundle("typescript/shell.ts", "esm", "shell.mjs"), "assets/feed-worker.js": bundle("typescript/feed-worker.ts", "iife", "feed-worker.js") };
  for (const [committed, built] of Object.entries(artifacts)) {
    const actual = readFileSync(built);
    if (process.argv.includes("--write")) writeFileSync(join(root, committed), actual);
    else if (!actual.equals(readFileSync(join(root, committed)))) throw new Error(`${committed} differs. Run node crates/web-shell/build.mjs --write and review the diff.`);
  }
  if (process.argv.includes("--test")) {
    const env = {
      ...process.env,
      SHELL_MODULE: artifacts["assets/shell.js"],
      CORE_MODULE: bundle("typescript/core.ts", "esm", "core.mjs"),
      FEED_MODULE: bundle("typescript/feed.ts", "esm", "feed.mjs"),
      TIMELINE_MODULE: bundle("typescript/timeline.ts", "esm", "timeline.mjs"),
    };
    const tests = readdirSync(join(root, "tests")).filter(name => name.endsWith(".test.mjs")).sort().map(name => join("tests", name));
    run(process.execPath, ["--test", ...tests], { env });
  }
} finally { rmSync(scratch, { recursive: true, force: true }); }
