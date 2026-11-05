// Run with: node --test apps/desktop/scripts/judge-smoke.test.mjs
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("judge-smoke.mjs", import.meta.url));

const check = (id, measured, limit) => ({ id, metric: id, measured, limit, status: "pass", note: "" });

/** The judge's output for a second launch that the app measured against its own 3000 ms limit. */
function judge(coldStartMs) {
  const dir = mkdtempSync(join(tmpdir(), "brigadier-judge-smoke-"));
  try {
    const report = join(dir, "launch.json");
    writeFileSync(report, JSON.stringify({ checks: [check("firstColdStart", 2000, 9000), check("coldStart", coldStartMs, 3000)] }));
    const combined = join(dir, "combined.json");
    const { stdout, status } = spawnSync(process.execPath, [script, combined, report], { encoding: "utf8" });
    return { stdout, status, combined: JSON.parse(readFileSync(combined, "utf8")) };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("a cold start under 3500 ms is no regression, whatever limit the app applied (#679)", () => {
  const { stdout, status, combined } = judge(3199);
  assert.equal(status, 0);
  assert.doesNotMatch(stdout, /PERF REGRESSION/);
  assert.equal(combined.coldStart.limitMs, 3500);
});

test("a cold start at 3500 ms or more is still reported", () => {
  assert.match(judge(3500).stdout, /^PERF REGRESSION: Cold start 3500 vs limit 3500$/m);
});
