import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { accessSync, constants, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";

// Use the existing browser installation, or CHROME_BIN on a verifier's machine. No downloads.
function chromium(): string {
  const candidates = [
    process.env.CHROME_BIN,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/google-chrome",
  ];
  for (const cache of [join(homedir(), "Library/Caches/ms-playwright"), join(homedir(), ".cache/ms-playwright")]) {
    let entries: string[];
    try {
      entries = readdirSync(cache);
    } catch {
      continue;
    }
    for (const entry of entries.filter((name) => name.startsWith("chromium"))) {
      for (const binary of [
        "chrome-headless-shell-mac-arm64/chrome-headless-shell",
        "chrome-headless-shell-mac-x64/chrome-headless-shell",
        "chrome-headless-shell-linux64/chrome-headless-shell",
        "chrome-linux/chrome",
      ]) candidates.push(join(cache, entry, binary));
    }
  }
  for (const candidate of candidates) {
    if (!candidate) continue;
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {
      continue;
    }
  }
  throw new Error("Worker activity rendering test needs Chromium. Set CHROME_BIN to an installed Chrome/Chromium.");
}

type Row = { text: string; state: string; lines: string[]; subrows: number };
type Rendering = { thread: Record<string, Row>; strip: Record<string, Row> };

// Vite serves the real React components, and Chromium clicks the real Collapsible trigger.
// Disposable profiles and Vite's cache use the system temporary folder, never the app's data.
test("worker lifecycle stays compact while the expanded background strip shows activity", { timeout: 60000 }, async (t) => {
  const binary = chromium();
  const scratch = mkdtempSync(join(tmpdir(), "worker-render-"));
  // Keep Vite outside the pure-module test loader, which only resolves TypeScript imports.
  const config = {
    cacheDir: join(scratch, "vite-cache"),
    logLevel: "error",
    server: { host: "127.0.0.1", port: 0, strictPort: false, hmr: false, watch: null },
  };
  const server = spawn(process.execPath, ["--input-type=module", "--eval", `
    import { createServer } from "vite";
    const server = await createServer(${JSON.stringify(config)});
    await server.listen();
    console.log(server.resolvedUrls.local[0]);
    process.on("SIGTERM", async () => { await server.close(); process.exit(0); });
  `], { stdio: ["ignore", "pipe", "pipe"] });
  let serverErrors = "";
  server.stderr.on("data", (chunk: Buffer) => { serverErrors += chunk.toString(); });
  t.after(async () => {
    if (server.exitCode === null) {
      const stopped = new Promise((resolve) => server.once("exit", resolve));
      server.kill();
      await stopped;
    }
    rmSync(scratch, { recursive: true, force: true });
    assert.equal(serverErrors, "", "Vite must render without compilation errors");
  });
  const url = await new Promise<string>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Vite startup timed out: ${serverErrors}`)), 15000);
    let output = "";
    server.stdout.on("data", (chunk: Buffer) => {
      output += chunk.toString();
      const address = output.match(/http:\/\/127\.0\.0\.1:\d+\//)?.[0];
      if (address) {
        clearTimeout(timeout);
        resolve(address);
      }
    });
    server.once("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    server.once("exit", (code) => {
      clearTimeout(timeout);
      reject(new Error(`Vite exited ${code}: ${serverErrors}`));
    });
  });
  const { stdout } = await promisify(execFile)(binary, [
    "--headless", "--no-sandbox", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
    `--user-data-dir=${join(scratch, "profile")}`,
    "--dump-dom", "--virtual-time-budget=5000",
    `${url}fixtures/worker-activity.html`,
  ], { timeout: 45000, maxBuffer: 4 * 1024 * 1024 });
  const serialized = stdout.match(/<pre id="worker-activity-result">([^<]+)<\/pre>/)?.[1];
  assert.ok(serialized, `Fixture did not render its result:\n${stdout}`);
  const rendering = JSON.parse(serialized) as Rendering;
  assert.equal(Object.keys(rendering.strip).length, 4, "The strip must be expanded with all four active workers");

  await t.test("active rows render activity, elapsed time and diff", () => {
    assert.deepEqual(rendering.thread.active?.lines, []);
    assert.equal(rendering.thread.active?.subrows, 0);
    for (const rows of [rendering.strip]) {
      assert.deepEqual(rows.active?.lines, ["Editing apps/desktop/src/composer/Paste.tsx · 3m 12s · +209 −102"]);
      assert.equal(rows.active?.subrows, 1);
    }
  });
  await t.test("blocked, quota and queued rows render wait reasons instead of Working", () => {
    const waits = {
      blocked: "Waiting for a free worker · 3m 12s",
      quota: "Waiting for Codex quota: Codex weekly quota resets tomorrow · 40s",
      queued: "Queued: waiting for step 1 · 3m 12s",
    };
    for (const id of Object.keys(waits)) assert.deepEqual(rendering.thread[id]?.lines, []);
    for (const rows of [rendering.strip]) {
      for (const [id, expected] of Object.entries(waits)) {
        assert.deepEqual(rows[id]?.lines, [expected]);
        assert.doesNotMatch(rows[id]!.text, /Working/);
      }
    }
    assert.equal(rendering.thread.blocked?.state, "is waiting");
    assert.equal(rendering.thread.quota?.state, "is waiting for quota");
    assert.equal(rendering.thread.queued?.state, "is queued");
  });
  await t.test("completed rows render no activity subrow", () => {
    assert.match(rendering.thread.completed!.text, /finished/);
    assert.deepEqual(rendering.thread.completed?.lines, []);
    assert.equal(rendering.thread.completed?.subrows, 0);
    assert.equal(rendering.strip.completed, undefined);
  });
});
