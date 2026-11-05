// Keeps one tracking issue per metric from the "PERF REGRESSION" lines the smoke checks printed:
// a metric that missed opens its issue, or comments on the one already open.
//
//   node apps/desktop/scripts/perf-report.mjs [--close-fixed] <smoke.log>...
//
// With --close-fixed, given every platform's log, a run whose smoke checks all finished closes
// the issue of each metric that didn't miss. A log that's missing or stops short (a platform
// failed or was skipped) says nothing about its metrics, so then nothing closes. Never a gate:
// see §5 of docs/STRIP-DOWN-PLAN.md.
//
// Run its test with: node --test apps/desktop/scripts/perf-report.test.mjs
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const PREFIX = "Nightly perf regression: ";

export function report({ logs, closeFixed, gh, repo, runUrl }) {
  const lines = logs.flatMap((log) => (log ?? "").split(/\r?\n/));
  const missed = new Map();
  for (const line of new Set(lines)) {
    const metric = line.match(/^PERF REGRESSION: (.*) \S+ vs limit /)?.[1];
    if (metric) missed.set(metric, [...(missed.get(metric) ?? []), line]);
  }

  const open = JSON.parse(gh("issue", "list", "-R", repo, "--state", "open", "--limit", "1000", "--json", "number,title"))
    .filter((issue) => issue.title.startsWith(PREFIX));
  for (const [metric, metricLines] of missed) {
    const title = PREFIX + metric;
    const body = `${runUrl}\n\n${metricLines.join("\n")}`;
    const issue = open.find((candidate) => candidate.title === title);
    if (issue) gh("issue", "comment", String(issue.number), "-R", repo, "--body", body);
    else gh("issue", "create", "-R", repo, "--title", title, "--body", body);
  }

  const finished = logs.every((log) => log?.split(/\r?\n/).includes("Smoke check passed."));
  if (!closeFixed || !finished) return;
  for (const issue of open) {
    if (missed.has(issue.title.slice(PREFIX.length))) continue;
    gh("issue", "close", String(issue.number), "-R", repo, "--comment", `Within its limit on every platform: ${runUrl}`);
  }
}

function read(path) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  report({
    logs: args.filter((arg) => arg !== "--close-fixed").map(read),
    closeFixed: args.includes("--close-fixed"),
    gh: (...cli) => execFileSync("gh", cli, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }),
    repo: process.env.GITHUB_REPOSITORY,
    runUrl: process.env.RUN_URL,
  });
}
