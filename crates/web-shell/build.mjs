#!/usr/bin/env node
// No npm installation. Use the toolchain supplied by the development shell.
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { resolve, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
const root = dirname(fileURLToPath(import.meta.url));
const scratch = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), "web-shell-"));
function run(program, args, extra = {}) {
  const result = spawnSync(program, args, { cwd: root, stdio: "inherit", ...extra });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${program} failed (${result.status})`);
}
try {
  run(process.env.TSC ?? "tsc", ["--project", join(root, "tsconfig.json")]);
  const bundle = join(scratch, "shell.mjs");
  run(process.env.ESBUILD ?? "esbuild", ["typescript/shell.ts", "--bundle", "--format=esm", "--target=es2022", `--outfile=${bundle}`]);
  // esbuild's output depends only on source and its version; output filename is absent.
  const actual = readFileSync(bundle);
  const committed = join(root, "assets/shell.js");
  if (process.argv.includes("--write")) writeFileSync(committed, actual);
  else if (!actual.equals(readFileSync(committed))) throw new Error("shell.js differs. Run node crates/web-shell/build.mjs --write and review the diff.");
  if (process.argv.includes("--test")) {
    const core = join(scratch, "core.mjs");
    run(process.env.ESBUILD ?? "esbuild", ["typescript/core.ts", "--bundle", "--format=esm", "--target=es2022", `--outfile=${core}`]);
    run(process.execPath, ["--test", "tests/browser.test.mjs"], { env: { ...process.env, CORE_MODULE: core, SHELL_MODULE: bundle } });
  }
} finally { rmSync(scratch, { recursive: true, force: true }); }
