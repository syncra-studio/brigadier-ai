// Runs the app's tests with `node --test` (every src/**/*.test.ts, or the files named), passing
// on any options given, and turns a hang into a named failure: a test with no timeout of its own
// fails after three minutes, a file whose tests are all done ends even if something it started
// still holds it open, and a run that prints nothing for five minutes is stopped, which has the
// runner name the test files still running. A whole run prints a result at least every half
// minute or so.
//
// Under CI (CI=true) a test file that fails is run once more, and the run passes if it passes
// then: shared runners make some render tests miss a timing now and then. A test that fails both
// tries is skipped in its file and listed in the app-test flakes issue.
import { spawn } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";

const SILENT_MS = 5 * 60_000;
const given = process.argv.slice(2);
const named = given.some((arg) => /\.test\.[cm]?[jt]s$/.test(arg));
const retry = process.env.CI === "true";

/** One `node --test` run of `files`: its exit code and, under CI, the files with a failed test. */
function runOnce(files) {
  const failedList = join(tmpdir(), `brigadier-failed-tests-${process.pid}-${Date.now()}.json`);
  const reporters = retry
    ? ["--test-reporter=spec", "--test-reporter-destination=stdout",
       "--test-reporter=./scripts/failed-files-reporter.mjs", `--test-reporter-destination=${failedList}`]
    : [];
  const run = spawn(process.execPath, [
    "--import", "./scripts/test-alias.mjs", "--test", "--test-timeout=180000", "--test-force-exit", ...reporters,
    ...files,
  ], { stdio: ["inherit", "pipe", "pipe"] });

  let heard = Date.now();
  let stopped = false;
  for (const [from, to] of [[run.stdout, process.stdout], [run.stderr, process.stderr]]) {
    from.on("data", (chunk) => {
      heard = Date.now();
      to.write(chunk);
    });
  }
  const watch = setInterval(() => {
    if (Date.now() - heard < SILENT_MS) return;
    clearInterval(watch);
    stopped = true;
    console.error(`\nNo test printed anything for ${SILENT_MS / 60_000} minutes, so the run stops here. The files still running are named below.`);
    run.kill("SIGTERM");
    setTimeout(() => run.kill("SIGKILL"), 30_000).unref();
  }, 5_000);
  return new Promise((resolve) => {
    run.on("exit", (code) => {
      clearInterval(watch);
      let failed = [];
      try {
        failed = JSON.parse(readFileSync(failedList, "utf8"));
      } catch {
        // No list: not under CI, or the run was stopped before the reporter finished.
      }
      rmSync(failedList, { force: true });
      resolve({ code: stopped ? 1 : code ?? 1, failed });
    });
  });
}

const first = await runOnce([...given, ...(named ? [] : ["src/**/*.test.ts"])]);
if (first.code === 0 || !retry || first.failed.length === 0) process.exit(first.code);

const files = first.failed.map((file) => relative(process.cwd(), file));
const options = given.filter((arg) => !/\.test\.[cm]?[jt]s$/.test(arg));
console.log(`\nTrying once more the files with a failed test: ${files.join(", ")}\n`);
const second = await runOnce([...options, ...files]);
console.log(
  second.code === 0
    ? `\nThey passed on the second try: ${files.join(", ")}`
    : `\nFailed on both tries: ${second.failed.map((file) => relative(process.cwd(), file)).join(", ") || files.join(", ")}`,
);
process.exit(second.code);
