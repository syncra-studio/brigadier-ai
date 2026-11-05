// Run with: node --test apps/desktop/scripts/perf-report.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";

import { report } from "./perf-report.mjs";

const PASSED = "Smoke check passed.";
const COLD = "PERF REGRESSION: Cold start 3600 vs limit 3500";
const DELAY = "PERF REGRESSION: Runtime scheduler delay (proxy) 67 vs limit 30";

/** The `gh` calls that change something, given the issues open now. */
function run({ logs, open = [], closeFixed = true }) {
  const calls = [];
  const gh = (...args) => {
    if (args[1] === "list") return JSON.stringify(open);
    calls.push(args);
    return "";
  };
  report({ logs, closeFixed, gh, repo: "o/r", runUrl: "https://run" });
  return calls.map((args) => args.slice(0, 2).concat(args[2] === "-R" ? args[args.indexOf("--title") + 1] ?? [] : args[2]).join(" "));
}

const COLD_ISSUE = { number: 679, title: "Nightly perf regression: Cold start" };
const DELAY_ISSUE = { number: 687, title: "Nightly perf regression: Runtime scheduler delay (proxy)" };
const OTHER_ISSUE = { number: 5, title: "Nightly build failed" };

test("each metric gets its own issue, never one for the combination (#657)", () => {
  assert.deepEqual(run({ logs: [`${COLD}\n${DELAY}\n${PASSED}\n`] }), [
    "issue create Nightly perf regression: Cold start",
    "issue create Nightly perf regression: Runtime scheduler delay (proxy)",
  ]);
});

test("a metric that still misses comments on its open issue, once across platforms", () => {
  assert.deepEqual(run({ logs: [`${COLD}\n${PASSED}\n`, `${COLD}\r\n${PASSED}\r\n`], open: [COLD_ISSUE] }), ["issue comment 679"]);
});

test("a good run closes a metric's issue and leaves the others alone (#679, #687)", () => {
  assert.deepEqual(run({ logs: [`${DELAY}\n${PASSED}\n`, `${PASSED}\n`], open: [COLD_ISSUE, DELAY_ISSUE, OTHER_ISSUE] }), [
    "issue comment 687",
    "issue close 679",
  ]);
});

test("nothing closes when a platform has no log or its smoke check didn't finish", () => {
  const open = [COLD_ISSUE];
  assert.deepEqual(run({ logs: [`${PASSED}\n`, null], open }), []);
  assert.deepEqual(run({ logs: [`${PASSED}\n`, "FAIL: launch 1 produced no report\n"], open }), []);
});

test("a run that doesn't close, as CI's Linux-only one, still reports", () => {
  assert.deepEqual(run({ logs: [`${DELAY}\n${PASSED}\n`], open: [COLD_ISSUE], closeFixed: false }), [
    "issue create Nightly perf regression: Runtime scheduler delay (proxy)",
  ]);
});
