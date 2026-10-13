import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { accessSync, constants, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { TestContext } from "node:test";

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
 * Serves the fixtures through Vite until the test ends: its address, a scratch folder in the
 * system temporary folder (never the app's data), and what Vite logged.
 */
async function serveFixtures(t: TestContext): Promise<{ url: string; scratch: string; log: () => string }> {
  const scratch = mkdtempSync(join(tmpdir(), "fixture-render-"));
  // Keep Vite outside the pure-module test loader, which only resolves TypeScript imports.
  const config = {
    cacheDir: join(scratch, "vite-cache"),
    logLevel: "error",
    // Vite forwards the page's console to its log when it detects an agent's shell (AI_AGENT),
    // which would read as a compilation error here: the page's console stays its own.
    server: { host: "127.0.0.1", port: 0, strictPort: false, hmr: false, watch: null, forwardConsole: false },
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
    if (server.exitCode === null && server.signalCode === null) {
      const stopped = new Promise((resolve) => server.once("exit", resolve));
      server.kill();
      // Vite's close can wait on a request still being served: it goes outright after a moment,
      // so the test (and the file's run) always ends.
      const stuck = setTimeout(() => server.kill("SIGKILL"), 3000);
      await stopped;
      clearTimeout(stuck);
    }
    // Its pipes too, which a process it started could otherwise hold open.
    server.stdout.destroy();
    server.stderr.destroy();
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
  return { url, scratch, log: () => serverOutput };
}

/** The requests Vite started and never answered, from its log. */
function unanswered(log: string): string[] {
  const open = new Map<string, number>();
  for (const [, mark, path] of log.matchAll(/^([<>]) (.*)$/gm)) open.set(path!, (open.get(path!) ?? 0) + (mark === ">" ? 1 : -1));
  return [...open].filter(([, count]) => count > 0).map(([path]) => path);
}

/**
 * Renders `fixtures/<page>` in headless Chromium through Vite and returns the text of its
 * `<pre id=resultId>`, the fixture's own JSON. Profiles and Vite's cache use the system
 * temporary folder, never the app's data.
 */
export async function renderFixturePage(t: TestContext, page: string, resultId: string, budgetMs = 5000): Promise<string> {
  const binary = chromium();
  const { url, scratch, log } = await serveFixtures(t);
  // One process on macOS: a worker's sandbox lets Chromium look up Mach services but not register
  // the one its child processes rendezvous on, so the multi-process browser aborts there. Linux
  // Chrome crashes (SIGTRAP) in single-process mode, so it keeps the default.
  const oneProcess = process.platform === "darwin" ? ["--single-process"] : [];
  // Chrome stopped at the timeout exits cleanly with no DOM, so the failure names the time it ran.
  // Scrollbars hidden: Linux Chrome's take room where macOS's float, and the page lays out alike.
  const started = Date.now();
  // In its own process group, which goes whole at the timeout and after the test: a process
  // Chrome started would otherwise keep its output open, and this waiting, on and on.
  const chrome = spawn(binary, [
    "--headless", "--no-sandbox", ...oneProcess, "--disable-gpu", "--hide-scrollbars", "--no-first-run", "--no-default-browser-check",
    "--enable-logging=stderr",
    `--user-data-dir=${join(scratch, "profile")}`,
    "--dump-dom", `--virtual-time-budget=${budgetMs}`,
    `${url}fixtures/${page}`,
  ], { stdio: ["ignore", "pipe", "pipe"], detached: true });
  let stdout = "";
  let stderr = "";
  chrome.stdout.on("data", (chunk: Buffer) => { stdout += chunk.toString(); });
  chrome.stderr.on("data", (chunk: Buffer) => { stderr += chunk.toString(); });
  const killGroup = () => {
    try {
      process.kill(-chrome.pid!, "SIGKILL");
    } catch {
      // None of them is left.
    }
  };
  const late = setTimeout(killGroup, 45000);
  t.after(() => {
    clearTimeout(late);
    killGroup();
  });
  // How it ended, unless cleanly: its signal, or its exit code.
  const ended = await new Promise<string | null>((resolve, reject) => {
    chrome.once("error", reject);
    chrome.once("exit", (code, signal) => resolve(code === 0 ? null : signal ?? `exit code ${code}`));
  });
  clearTimeout(late);
  // What it started goes too, and then its output is all in.
  killGroup();
  await new Promise((resolve) => {
    if (chrome.stdout.readableEnded && chrome.stderr.readableEnded) resolve(null);
    else chrome.once("close", resolve);
  });
  // One-process Chromium on macOS can trap in its compositor thread while it shuts down, after
  // it printed the whole DOM (often after a dialog opened and closed): the page's result
  // stands. Any other unclean end, a timeout included, is a failure.
  if (ended !== null && !(oneProcess.length > 0 && ended === "SIGTRAP" && stdout.includes(`<pre id="${resultId}">`))) {
    assert.fail([
      `Chrome ended (${ended}) after ${Date.now() - started} ms of its 45000.`,
      `Requests Vite never answered: ${unanswered(log()).join(", ") || "none"}`,
      `Chrome's log:\n${stderr.slice(-4000)}`,
    ].join("\n"));
  }
  const serialized = stdout.match(new RegExp(`<pre id="${resultId}">([^<]+)</pre>`))?.[1];
  if (!serialized) {
    assert.fail([
      `Fixture did not render its result; Chrome ran ${Date.now() - started} ms of its 45000.`,
      `Requests Vite never answered: ${unanswered(log()).join(", ") || "none"}`,
      `Chrome's log:\n${stderr.slice(-4000)}`,
      `DOM:\n${stdout}`,
    ].join("\n"));
  }
  return serialized;
}

/**
 * Like `renderFixturePage`, for a fixture that measures motion: Chromium runs in real time (its
 * frames, animation callbacks and resize observers run as in the app, which virtual time stops),
 * in a 1280×976 window, and the result is read over the DevTools protocol once it shows.
 */
export async function renderFixtureLive(t: TestContext, page: string, resultId: string, timeoutMs = 60000, reducedMotion = false): Promise<string> {
  const binary = chromium();
  const { url, scratch, log } = await serveFixtures(t);
  const profile = join(scratch, "profile");
  const oneProcess = process.platform === "darwin" ? ["--single-process"] : [];
  const browser = spawn(binary, [
    "--headless", "--no-sandbox", ...oneProcess, "--disable-gpu", "--hide-scrollbars", "--no-first-run", "--no-default-browser-check",
    "--window-size=1280,976", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"], detached: true });
  let browserLog = "";
  browser.stderr.on("data", (chunk: Buffer) => { browserLog += chunk.toString(); });
  // A throwaway browser on a scratch profile: killed outright (one process ignores SIGTERM), with
  // the processes it started (its own group: Linux Chrome's renderers and helpers).
  t.after(async () => {
    if (browser.exitCode === null && browser.signalCode === null) {
      const stopped = new Promise((resolve) => browser.once("exit", resolve));
      process.kill(-browser.pid!, "SIGKILL");
      await stopped;
    } else {
      try {
        process.kill(-browser.pid!, "SIGKILL");
      } catch {
        // None of them is left.
      }
    }
    browser.stderr.destroy();
  });
  const deadline = Date.now() + timeoutMs;
  const port = await new Promise<number>((resolve, reject) => {
    const poll = setInterval(() => {
      try {
        const found = Number(readFileSync(join(profile, "DevToolsActivePort"), "utf8").split("\n")[0]);
        if (found > 0) {
          clearInterval(poll);
          resolve(found);
        }
      } catch {
        if (Date.now() > deadline || browser.exitCode !== null) {
          clearInterval(poll);
          reject(new Error(`Chromium did not start: ${browserLog.slice(-2000)}`));
        }
      }
    }, 100);
  });
  const targets = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()) as { type: string; webSocketDebuggerUrl: string }[];
  const target = targets.find((candidate) => candidate.type === "page");
  assert.ok(target, "Chromium has a page");
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  t.after(() => socket.close());
  let id = 0;
  const replies = new Map<number, (value: unknown) => void>();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data)) as { id?: number; result?: unknown };
    if (message.id !== undefined) replies.get(message.id)?.(message.result);
  });
  // A call the browser never answers (it stopped, or its socket closed) fails at the deadline
  // instead of waiting on.
  let closed = false;
  socket.addEventListener("close", () => { closed = true; });
  const call = (method: string, params: object) =>
    new Promise<unknown>((resolve, reject) => {
      if (closed) return reject(new Error(`Chromium's connection closed before ${method}`));
      const asked = ++id;
      const late = setTimeout(() => {
        replies.delete(asked);
        reject(new Error(`Chromium didn't answer ${method} by the deadline`));
      }, Math.max(deadline - Date.now(), 1000));
      replies.set(asked, (value) => {
        clearTimeout(late);
        replies.delete(asked);
        resolve(value);
      });
      socket.send(JSON.stringify({ id: asked, method, params }));
    });
  // What a page can't do itself, done for it as a user's input would be (`fixtureInput`, in
  // fixtures/input.ts): a wheel turned over a point, or the window resized.
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data)) as { method?: string; params?: { name: string; payload: string } };
    if (message.method !== "Runtime.bindingCalled" || message.params?.name !== "fixtureInput") return;
    const asked = JSON.parse(message.params.payload) as
      { id: number; type: "wheel"; x: number; y: number; deltaX?: number; deltaY?: number } | { id: number; type: "viewport"; width: number; height: number };
    const done = () => call("Runtime.evaluate", { expression: `window.fixtureInputDone?.(${asked.id})` });
    const act = asked.type === "wheel"
      ? call("Input.dispatchMouseEvent", { type: "mouseWheel", x: asked.x, y: asked.y, deltaX: asked.deltaX ?? 0, deltaY: asked.deltaY ?? 0 })
      : call("Emulation.setDeviceMetricsOverride", { width: asked.width, height: asked.height, deviceScaleFactor: 1, mobile: false });
    void act.then(done);
  });
  await call("Runtime.enable", {});
  await call("Runtime.addBinding", { name: "fixtureInput" });
  // The page acts as the focused one, as a window in front is: headless Chrome gives none the
  // window's focus, so a field's focus() would fire no focus event.
  await call("Emulation.setFocusEmulationEnabled", { enabled: true });
  if (reducedMotion) await call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  await call("Page.navigate", { url: `${url}fixtures/${page}` });
  while (Date.now() < deadline) {
    const { result } = (await call("Runtime.evaluate", {
      expression: `document.getElementById(${JSON.stringify(resultId)})?.textContent ?? null`,
      returnByValue: true,
    })) as { result: { value: string | null } };
    if (result.value) return result.value;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  return assert.fail([
    `Fixture did not render its result in ${timeoutMs} ms.`,
    `Requests Vite never answered: ${unanswered(log()).join(", ") || "none"}`,
    `Chrome's log:\n${browserLog.slice(-4000)}`,
  ].join("\n"));
}

