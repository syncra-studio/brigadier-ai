// Runs the app's tests with `node --test` (every src/**/*.test.ts, or the files named), passing
// on any options given, and turns a hang into a named failure: a test with no timeout of its own
// fails after three minutes, a file whose tests are all done ends even if something it started
// still holds it open, and a run that prints nothing for five minutes is stopped, which has the
// runner name the test files still running. A whole run prints a result at least every half
// minute or so.
import { spawn } from "node:child_process";

const SILENT_MS = 5 * 60_000;
const given = process.argv.slice(2);
const named = given.some((arg) => /\.test\.[cm]?[jt]s$/.test(arg));
const run = spawn(process.execPath, [
  "--import", "./scripts/test-alias.mjs", "--test", "--test-timeout=180000", "--test-force-exit", ...given,
  ...(named ? [] : ["src/**/*.test.ts"]),
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
run.on("exit", (code) => {
  clearInterval(watch);
  process.exit(stopped ? 1 : code ?? 1);
});
