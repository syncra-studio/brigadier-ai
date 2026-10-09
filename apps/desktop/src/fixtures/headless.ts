import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { accessSync, constants, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { TestContext } from "node:test";
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
  throw new Error("Fixture rendering tests need Chromium. Set CHROME_BIN to an installed Chrome/Chromium.");
}

/**
 * Renders `fixtures/<page>` in headless Chromium through Vite and returns the text of its
 * `<pre id=resultId>`, the fixture's own JSON. Profiles and Vite's cache use the system
 * temporary folder, never the app's data.
 */
export async function renderFixturePage(t: TestContext, page: string, resultId: string, budgetMs = 5000): Promise<string> {
  const binary = chromium();
  const scratch = mkdtempSync(join(tmpdir(), "fixture-render-"));
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
    const requests = { name: "requests", configureServer(server) {
      server.middlewares.use((request, response, next) => {
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
    "--dump-dom", `--virtual-time-budget=${budgetMs}`,
    `${url}fixtures/${page}`,
  ], { timeout: 45000, maxBuffer: 4 * 1024 * 1024 });
  const serialized = stdout.match(new RegExp(`<pre id="${resultId}">([^<]+)</pre>`))?.[1];
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
  return serialized;
}
