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
  throw new Error("Mentions test needs Chromium. Set CHROME_BIN to an installed Chrome/Chromium.");
}

// Vite serves the real mention menu; its IPC stub lists a disposable checkout on disk.
// Disposable profiles and Vite's cache use the system temporary folder, never the app's data.
test("the mention menu refreshes checkout files on opening, not on each keystroke", { timeout: 60000 }, async (t) => {
  const binary = chromium();
  const scratch = mkdtempSync(join(tmpdir(), "mentions-refresh-"));
  // Keep Vite outside the pure-module test loader, which only resolves TypeScript imports.
  const config = {
    cacheDir: join(scratch, "vite-cache"),
    logLevel: "error",
    server: { host: "127.0.0.1", port: 0, strictPort: false, hmr: false, watch: null },
  };
  // Vite logs each request's start (>) and end (<), so a page that never finishes loading names
  // the request it waits on.
  const server = spawn(process.execPath, ["--input-type=module", "--eval", `
    import { createServer } from "vite";
    import { execFileSync } from "node:child_process";
    import { mkdirSync, writeFileSync } from "node:fs";
    import { join } from "node:path";
    const checkout = join(${JSON.stringify(scratch)}, "checkout");
    mkdirSync(checkout);
    execFileSync("git", ["init", "--quiet", checkout]);
    let failNext = false;
    const requests = { name: "requests", configureServer(server) {
      server.middlewares.use((request, response, next) => {
        if (request.url === "/mention-test/fail-next") {
          failNext = true;
          response.end("ready");
          return;
        }
        if (request.url === "/mention-test/create") {
          writeFileSync(join(checkout, "hello.txt"), "hello");
          response.end("created");
          return;
        }
        if (request.url === "/mention-test/files") {
          if (failNext) {
            failNext = false;
            response.statusCode = 503;
            response.end("Listing unavailable");
            return;
          }
          const files = execFileSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], { cwd: checkout, encoding: "utf8" }).split("\\0").filter(Boolean);
          response.setHeader("Content-Type", "application/json");
          response.end(JSON.stringify({ method: "listFiles", files, truncated: false }));
          return;
        }
        console.log("> " + request.url);
        response.on("close", () => console.log("< " + request.url));
        next();
      });
    } };
    const server = await createServer({ ...${JSON.stringify(config)}, plugins: [requests] });
    await server.listen();
    console.log(server.resolvedUrls.local[0]);
    process.on("SIGTERM", async () => { await server.close(); process.exit(0); });
  `], { stdio: ["ignore", "pipe", "pipe"] });
  let serverErrors = "";
  let serverOutput = "";
  server.stdout.on("data", (chunk: Buffer) => { serverOutput += chunk.toString(); });
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
    server.stdout.on("data", () => {
      const address = serverOutput.match(/http:\/\/127\.0\.0\.1:\d+\//)?.[0];
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
  // One process on macOS: a worker's sandbox lets Chromium look up Mach services but not register
  // the one its child processes rendezvous on, so the multi-process browser aborts there. Linux
  // Chrome crashes (SIGTRAP) in single-process mode, so it keeps the default.
  const oneProcess = process.platform === "darwin" ? ["--single-process"] : [];
  // Chrome stopped at the timeout exits cleanly with no DOM, so the failure names the time it ran.
  const started = Date.now();
  const { stdout, stderr } = await promisify(execFile)(binary, [
    "--headless", "--no-sandbox", ...oneProcess, "--disable-gpu", "--no-first-run", "--no-default-browser-check",
    "--enable-logging=stderr",
    `--user-data-dir=${join(scratch, "profile")}`,
    "--dump-dom", "--virtual-time-budget=5000",
    `${url}fixtures/mentions-refresh.html`,
  ], { timeout: 45000, maxBuffer: 4 * 1024 * 1024 });
  const serialized = stdout.match(/<pre id="mentions-refresh-result">([^<]+)<\/pre>/)?.[1];
  if (!serialized) {
    const open = new Map<string, number>();
    for (const [, mark, path] of serverOutput.matchAll(/^([<>]) (.*)$/gm)) open.set(path!, (open.get(path!) ?? 0) + (mark === ">" ? 1 : -1));
    const unanswered = [...open].filter(([, count]) => count > 0).map(([path]) => path);
    assert.fail([
      `Fixture did not render its result; Chrome ran ${Date.now() - started} ms of its 45000.`,
      `Requests Vite never answered: ${unanswered.join(", ") || "none"}`,
      `Chrome's log:\n${stderr.slice(-4000)}`,
      `DOM:\n${stdout}`,
    ].join("\n"));
  }
  const result = JSON.parse(serialized) as { error?: string; calls: number; chatCalls: number; opens: number };
  assert.equal(result.error, undefined);
  assert.equal(result.calls, 4, "Mount and three menu openings each fetch once, including a failed refresh");
  assert.equal(result.chatCalls, 0, "Plain chats never list checkout files");
  assert.equal(result.opens, 2, "An inline onOpen fires once per opening despite rerenders");
});
