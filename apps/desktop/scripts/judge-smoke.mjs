// Judges the CI launch smoke check from several judged launches:
//
//   node scripts/judge-smoke.mjs <combined-report.json> <launch-1.json> <launch-2.json> ...
//
// The launches come in pairs: a first launch on a new data folder, then a second launch on the
// same folder. Each report measures the startup budget that fits it (first launch or cold
// start) and marks the other n/a. Each of the two is judged by the median of the launches that
// measured it against the unchanged limit (budget × tolerance): on shared runners most of it is
// the platform creating the window and webview before the page loads, and that swings by
// seconds between otherwise identical launches. Every other check is judged on the first
// launch exactly as the app judged it. All launches
// are printed with their startup milestones, and written to the combined report.
// Runner preparation may delay a launch, but every judged launch is kept.
// There is no retry or best-of selection: a failed launch remains part of the judgment.

import { readFileSync, writeFileSync } from "node:fs";

const [combinedPath, ...launchPaths] = process.argv.slice(2);
if (!combinedPath || launchPaths.length === 0) {
  console.error("usage: judge-smoke.mjs <combined-report.json> <launch.json>...");
  process.exit(2);
}

const launches = launchPaths.map((path) => {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    return { passed: false, error: `no report at ${path}: ${error.message}` };
  }
});

// A launch that produced no report (app did not launch, daemon not up, ...) still fails the
// job. A budget breach on a launch that did report is a performance limit, not a functional
// failure: docs/STRIP-DOWN-PLAN.md §5 makes benchmarks a nightly report, never a gate, so it
// only prints a "PERF REGRESSION" line for the workflow's regression-issue step to pick up.
const failures = [];
launches.forEach((launch, index) => {
  if (!Array.isArray(launch.checks)) {
    failures.push(`launch ${index + 1} produced no report (${launch.error ?? "unknown error"})`);
  }
});
const regression = (metric, measured, limit) =>
  console.log(`PERF REGRESSION: ${metric} ${Math.round(measured)} vs limit ${Math.round(limit)}`);

const STARTUP = { firstColdStart: "First launch", coldStart: "Cold start" };
const checkOf = (launch, id) => launch.checks?.find((check) => check.id === id);
const startup = {};
for (const [id, name] of Object.entries(STARTUP)) {
  const judged = launches
    .map((launch, index) => ({ index, check: checkOf(launch, id) }))
    .filter(({ check }) => check && check.status !== "notApplicable");
  const measured = judged.map(({ check }) => check.measured ?? null);
  const limit = judged[0]?.check.limit ?? null;
  const sorted = measured.filter((value) => typeof value === "number").toSorted((a, b) => a - b);
  const median =
    sorted.length > 0 && sorted.length === judged.length
      ? sorted[Math.floor(sorted.length / 2)]
      : null;
  startup[id] = { medianMs: median, limitMs: limit, launchesMs: measured };

  console.log(`${name} (limit ${limit} ms, judged by the median of ${judged.length} launches):`);
  for (const { index, check } of judged) {
    const value = check.measured == null ? "n/a" : `${Math.round(check.measured)} ms`;
    console.log(`  launch ${index + 1}: ${value}; ${check.note}`);
    for (const timing of ["frameGaps", "schedulerDelay"]) {
      const other = checkOf(launches[index], timing);
      console.log(
        `    ${timing}: ${other ? `${other.measured} ms (${other.status}, limit ${other.limit} ms)` : "n/a"}`,
      );
    }
  }
  console.log(`  median: ${median == null ? "n/a" : `${Math.round(median)} ms`}`);
  if (median == null || limit == null) {
    failures.push(
      `${name.toLowerCase()} median ${median == null ? "n/a" : Math.round(median)} ms has no limit to judge it against`,
    );
  } else if (!(median < limit)) {
    regression(name, median, limit);
  }
}

const first = launches[0];
for (const check of first.checks ?? []) {
  if (check.id in STARTUP) continue;
  console.log(`${check.metric}: ${check.status} (measured ${check.measured}, limit ${check.limit}) ${check.note}`);
  if (check.status === "fail") regression(check.metric, check.measured, check.limit);
}

const passed = failures.length === 0;
writeFileSync(
  combinedPath,
  `${JSON.stringify(
    {
      passed,
      failures,
      ...startup,
      launches,
    },
    null,
    2,
  )}\n`,
);
if (!passed) {
  for (const failure of failures) console.error(`FAIL: ${failure}`);
  process.exit(1);
}
console.log("Smoke check passed.");
