import assert from "node:assert/strict";
import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { accessSync, closeSync, constants, mkdtempSync, openSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { TestContext } from "node:test";

import type { FixtureInput } from "@/fixtures/input";

// Use the existing browser installation, or CHROME_BIN on a verifier's machine. No downloads.
function chromium(): string {
  const candidates = [
    process.env.CHROME_BIN,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/google-chrome",
  ];
  for (const cache of [
    join(homedir(), "Library/Caches/ms-playwright"),
    join(homedir(), ".cache/ms-playwright"),
    join(process.env.LOCALAPPDATA ?? homedir(), "ms-playwright"),
  ]) {
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
        "chrome-headless-shell-win64/chrome-headless-shell.exe",
      ]) candidates.push(join(cache, entry, binary));
    }
  }
  // Windows Chrome last: its headless mode keeps the scrollbars --hide-scrollbars hides in the
  // headless shell, and they take room there.
  candidates.push("C:/Program Files/Google/Chrome/Application/chrome.exe");
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

/** A Vite server on the fixtures: its address, its process, and the files its output goes to. */
export type FixtureServer = { url: string; pid: number; log: string; errors: string };

// Where a run's one server is announced to its test files (`globalSetup`, below).
const ANNOUNCED = "BRIGADIER_FIXTURE_SERVER";
let serversStarted = 0;

/**
 * Starts Vite on the fixtures, its dependency cache and output in a folder of its own (in the
 * system temporary folder, never the app's data, and gone if it can't start), and answers once its dependencies are optimized: a
 * render then loads the page, not a cold dev server's first optimization. It is a throwaway:
 * it goes outright (SIGKILL), as Vite's close can wait on a request still being served.
 */
async function startFixtureServer(): Promise<{ server: FixtureServer; child: ChildProcess; folder: string }> {
  const folder = mkdtempSync(join(tmpdir(), "fixture-server-"));
  // Keep Vite outside the pure-module test loader, which only resolves TypeScript imports.
  const config = {
    cacheDir: join(folder, "vite-cache"),
    logLevel: "error",
    // Vite forwards the page's console to its log when it detects an agent's shell (AI_AGENT),
    // which would read as a compilation error here: the page's console stays its own.
    server: { host: "127.0.0.1", port: 0, strictPort: false, hmr: false, watch: null, forwardConsole: false },
  };
  const log = join(folder, "vite.log");
  const errors = join(folder, "vite-errors.log");
  const out = openSync(log, "a");
  const err = openSync(errors, "a");
  // Vite logs each request's start (>) and end (<), so a page that never finishes loading names
  // the request it waits on, and a dependency optimized again mid-run (which would leave a page
  // loaded before it asking for files that are gone).
  const server = spawn(process.execPath, ["--input-type=module", "--eval", `
    import { createLogger, createServer } from "vite";
    const requests = { name: "requests", configureServer(server) {
      server.middlewares.use((request, response, next) => {
        console.log("> " + request.url);
        response.on("close", () => console.log("< " + request.url));
        next();
      });
    } };
    const logger = createLogger(${JSON.stringify(config.logLevel)});
    const info = (message) => { if (/dependencies optimized|optimized dependencies changed/.test(message)) console.log("optimizer: " + message); };
    const server = await createServer({ ...${JSON.stringify(config)}, customLogger: { ...logger, info }, plugins: [requests] });
    await server.listen();
    // Dependencies are optimized once the scan of every fixture's imports has run and its bundle
    // is written; no page waits on it after that.
    const optimizer = server.environments.client.depsOptimizer;
    await optimizer?.scanProcessing;
    await Promise.all(Object.values(optimizer?.metadata.discovered ?? {}).map((dep) => dep.processing));
    console.log("ready " + server.resolvedUrls.local[0]);
    // A run stopped outright (its teardown never ran) leaves no server behind. Windows never
    // changes a process's ppid, so there it asks whether the parent is still running.
    const parent = process.ppid;
    const gone = () => { try { process.kill(parent, 0); return false; } catch { return true; } };
    setInterval(() => { if (process.ppid !== parent || (process.platform === "win32" && gone())) process.exit(0); }, 1000).unref();
  `], { stdio: ["ignore", out, err], windowsHide: true });
  serversStarted += 1;
  closeSync(out);
  closeSync(err);
  const url = await new Promise<string>((resolve, reject) => {
    // Its first optimization takes a while on a busy machine.
    const deadline = Date.now() + 120000;
    // Until it's ready: once it is, its end (and its folder) is the run's to handle.
    const failed = (error: Error) => {
      clearInterval(poll);
      server.off("error", failed);
      server.off("exit", exited);
      rmSync(folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
      reject(error);
    };
    const exited = (code: number | null) => failed(new Error(`Vite exited ${code}: ${readFileSync(errors, "utf8")}`));
    const poll = setInterval(() => {
      const address = readFileSync(log, "utf8").match(/^ready (http:\/\/127\.0\.0\.1:\d+\/)$/m)?.[1];
      if (address) {
        clearInterval(poll);
        server.off("error", failed);
        server.off("exit", exited);
        resolve(address);
      } else if (Date.now() > deadline) {
        server.kill("SIGKILL");
        failed(new Error(`Vite startup timed out: ${readFileSync(errors, "utf8")}`));
      }
    }, 50);
    server.once("error", failed);
    server.once("exit", exited);
  });
  return { server: { url, pid: server.pid!, log, errors }, child: server, folder };
}

let own: Promise<FixtureServer> | undefined;

/** How many servers this process started: none in a run's test file, one in a file run alone. */
export function fixtureServersStarted(): number {
  return serversStarted;
}

/**
 * The fixtures' server: the run's one (`globalSetup`) or, for a file run on its own, one the
 * file's renders share, gone with its process.
 */
export function fixtureServer(): Promise<FixtureServer> {
  const announced = process.env[ANNOUNCED];
  if (announced) return Promise.resolve(JSON.parse(announced) as FixtureServer);
  own ??= (async () => {
    const { server, child, folder } = await startFixtureServer();
    // It doesn't keep the file's run going. When that ends, Vite goes outright, then its folder.
    child.unref();
    process.once("exit", () => {
      child.kill("SIGKILL");
      rmSync(folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    });
    return server;
  })();
  return own;
}

let run: { folder: string; child: ChildProcess } | undefined;

/**
 * `node --test --test-global-setup` (scripts/run-tests.mjs): one server, its dependencies already
 * optimized, for every render test file of the run, which finds it through the environment.
 */
export async function globalSetup(): Promise<void> {
  const { server, child, folder } = await startFixtureServer();
  run = { folder, child };
  process.env[ANNOUNCED] = JSON.stringify(server);
}

// All at once: `--test-force-exit` ends the process while a teardown still waits.
export function globalTeardown(): void {
  if (!run) return;
  run.child.kill("SIGKILL");
  rmSync(run.folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  run = undefined;
}

/** The size of a file of a server's output, then what it gained since. */
function sizeOf(file: string): number {
  return statSync(file).size;
}
function since(file: string, from: number): string {
  return readFileSync(file).subarray(from).toString();
}

/**
 * Serves the fixtures until the test ends: the server's address, a scratch folder in the system
 * temporary folder (never the app's data), what Vite logged since, and `stopFirst`, which names
 * what must be gone before the folder goes (the browser, whose profile is in it).
 */
async function serveFixtures(t: TestContext): Promise<{
  url: string;
  scratch: string;
  log: () => string;
  stopFirst: (stop: () => void | Promise<void>) => void;
}> {
  const server = await fixtureServer();
  const scratch = mkdtempSync(join(tmpdir(), "fixture-render-"));
  const stops: (() => void | Promise<void>)[] = [];
  const logFrom = sizeOf(server.log);
  const errorsFrom = sizeOf(server.errors);
  t.after(async () => {
    // The browser first: until it's gone it writes into its profile here, and the folder can't go.
    for (const stop of stops) await stop();
    // A process the browser started may still be closing its files: a few tries.
    rmSync(scratch, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    // Another file's render may share the server: an error in its time fails this one too.
    assert.equal(since(server.errors, errorsFrom), "", "Vite must render without compilation errors");
  });
  return { url: server.url, scratch, log: () => since(server.log, logFrom), stopFirst: (stop) => stops.push(stop) };
}

/** The requests Vite started and never answered, from its log. */
function unanswered(log: string): string[] {
  const open = new Map<string, number>();
  for (const [, mark, path] of log.matchAll(/^([<>]) (.*)$/gm)) open.set(path!, (open.get(path!) ?? 0) + (mark === ">" ? 1 : -1));
  return [...open].filter(([, count]) => count > 0).map(([path]) => path);
}

/**
 * Whether Chromium runs as one process: only where its multi-process browser can't start (a
 * sandbox, below). One process stops answering the DevTools protocol now and then, every call
 * after left unanswered, even with the machine idle: about 1 render in 6 of TurnErrorRender's
 * first test (#393). Linux Chrome crashes (SIGTRAP) in one process, and never needs it.
 */
let oneProcess = false;

/**
 * Kills `pid` outright with every process it started: its group, which it leads (`detached`).
 * Windows has no process groups to signal (`process.kill(-pid)` throws there), so taskkill ends
 * its process tree instead. Taskkill fails when one of the tree ends on its own meanwhile (a
 * renderer closing), which is no failure once `pid` itself is gone: it throws only for a process
 * still running.
 */
function killTree(pid: number): void {
  if (process.platform !== "win32") {
    process.kill(-pid, "SIGKILL");
    return;
  }
  const running = () => {
    try {
      process.kill(pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  for (let tries = 0; tries < 3; tries += 1) {
    if (!running()) return;
    spawnSync("taskkill", ["/pid", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
  }
  if (!running()) return;
  throw new Error(`taskkill could not end process ${pid}`);
}

/**
 * A headless Chromium on a scratch profile in `scratch`, driven over the DevTools protocol: its
 * page's socket, `call` (a call the browser never answers fails at `deadline`), its log, and
 * `stop`, which also runs when the test ends.
 */
async function openBrowser(
  t: TestContext,
  scratch: string,
  stopFirst: (stop: () => void | Promise<void>) => void,
  flags: string[],
  deadline: number,
): Promise<{ socket: WebSocket; call: (method: string, params?: object) => Promise<unknown>; log: () => string; stop: () => Promise<void> }> {
  const profile = join(scratch, "profile");
  // Scrollbars hidden: Linux Chrome's take room where macOS's float, and the page lays out alike.
  // A mouse, as the app's window has: Chrome on a machine with none (a CI runner) reports
  // `hover: none`, and every hover: style, which sits inside `@media (hover: hover)`, would stay off.
  const mouse = `--blink-settings=primaryHoverType=2,availableHoverTypes=2,primaryPointerType=4,availablePointerTypes=4`;
  // Its own process group (`detached`) where one can be signalled. Windows needs none (taskkill
  // ends the tree), and a detached process there has no console, so each process the headless
  // shell starts would open a console window of its own: it shares a hidden one instead.
  const launch = () => spawn(chromium(), [
    "--headless", "--no-sandbox", ...(oneProcess ? ["--single-process"] : []), "--disable-gpu", "--hide-scrollbars", mouse, "--no-first-run",
    "--no-default-browser-check", ...flags, "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"], detached: process.platform !== "win32", windowsHide: true });
  let browser = launch();
  let browserLog = "";
  const listen = () => browser.stderr.on("data", (chunk: Buffer) => { browserLog += chunk.toString(); });
  listen();
  // A throwaway browser on a scratch profile: killed outright (one process ignores SIGTERM), with
  // the processes it started (its own group: its renderers and helpers).
  const stop = async () => {
    if (browser.exitCode === null && browser.signalCode === null) {
      const stopped = new Promise((resolve) => browser.once("exit", resolve));
      killTree(browser.pid!);
      await stopped;
    } else {
      try {
        killTree(browser.pid!);
      } catch {
        // None of them is left.
      }
    }
    browser.stderr.destroy();
  };
  stopFirst(stop);
  const started = () => new Promise<number>((resolve, reject) => {
    const poll = setInterval(() => {
      try {
        const found = Number(readFileSync(join(profile, "DevToolsActivePort"), "utf8").split("\n")[0]);
        if (found > 0) {
          clearInterval(poll);
          resolve(found);
        }
      } catch {
        if (Date.now() > deadline || browser.exitCode !== null || browser.signalCode !== null) {
          clearInterval(poll);
          reject(new Error(`Chromium did not start: ${browserLog.slice(-2000)}`));
        }
      }
    }, 100);
  });
  let port: number;
  try {
    port = await started();
  } catch (error) {
    // A worker's macOS sandbox lets Chromium look up Mach services but not register the one its
    // child processes rendezvous on, so the multi-process browser aborts at once: one process
    // there, for the rest of the run.
    if (oneProcess || process.platform !== "darwin" || !browserLog.includes("MachPortRendezvous")) throw error;
    await stop();
    oneProcess = true;
    browserLog = "";
    browser = launch();
    listen();
    port = await started();
  }
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
  const call = (method: string, params: object = {}) =>
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
  return { socket, call, log: () => browserLog, stop };
}

/**
 * Renders `fixtures/<page>` in headless Chromium through Vite and returns the text of its
 * `<pre id=resultId>`, the fixture's own JSON, from the page's DOM once `budgetMs` of virtual
 * time have passed (as `--virtual-time-budget` does: its timers jump ahead whenever it is idle
 * and nothing is loading). Profiles and Vite's cache use the system temporary folder, never the
 * app's data.
 */
export async function renderFixturePage(t: TestContext, page: string, resultId: string, budgetMs = 5000): Promise<string> {
  const { url, scratch, log, stopFirst } = await serveFixtures(t);
  // Chrome stopped at the timeout leaves no DOM, so the failure names the time it ran.
  const started = Date.now();
  const deadline = started + 45000;
  const browser = await openBrowser(t, scratch, stopFirst, ["--enable-logging=stderr"], deadline);
  let expired = false;
  const spent = new Promise<void>((resolve) => {
    browser.socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as { method?: string };
      if (message.method === "Emulation.virtualTimeBudgetExpired") {
        expired = true;
        resolve();
      }
    });
  });
  const evaluate = async (expression: string) =>
    ((await browser.call("Runtime.evaluate", { expression, returnByValue: true })) as { result: { value: unknown } }).result.value;
  let stdout: string | null = null;
  let clock: unknown = null;
  let failure = "";
  try {
    // The page acts as the focused one, as `--dump-dom`'s did: Chrome (not its headless shell)
    // gives a tab opened over the DevTools protocol no focus, so focus() and keys would miss.
    await browser.call("Emulation.setFocusEmulationEnabled", { enabled: true });
    await browser.call("Emulation.setVirtualTimePolicy", { policy: "pause" });
    await browser.call("Page.navigate", { url: `${url}fixtures/${page}` });
    await browser.call("Emulation.setVirtualTimePolicy", { policy: "pauseIfNetworkFetchesPending", budget: budgetMs });
    // Nothing is asked while the budget runs: a one-process Chrome (a sandbox's) asked as the
    // page's clock races to the budget's end stops answering more often (#393: TurnErrorRender's
    // first test 1 render in 6 asked every 100 ms, 1 in 50 asked once spent).
    let waited: ReturnType<typeof setTimeout> | undefined;
    await Promise.race([spent, new Promise((resolve) => { waited = setTimeout(resolve, 10000); })]);
    clearTimeout(waited);
    while (Date.now() < deadline) {
      // The fixture's clock, not about:blank's before it. A loaded machine's one-process Chrome
      // can stop sending the page's events, the budget's end among them, while it still answers
      // (#375): the page's clock, stopped at the budget, says it is spent too.
      clock = await evaluate(`document.URL === "about:blank" ? null : performance.now()`);
      if (expired || (typeof clock === "number" && clock >= budgetMs)) {
        stdout = String(await evaluate("document.documentElement.outerHTML"));
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  } catch (error) {
    failure = String(error);
  }
  // Done with it: it goes now, not at the test's end.
  await browser.stop();
  if (stdout === null) {
    assert.fail([
      `Chrome ended after ${Date.now() - started} ms of its 45000, its page's clock at ${String(clock)} ms of ${budgetMs}. ${failure}`,
      `Requests Vite never answered: ${unanswered(log()).join(", ") || "none"}`,
      `Chrome's log:\n${browser.log().slice(-4000)}`,
    ].join("\n"));
  }
  const serialized = stdout.match(new RegExp(`<pre id="${resultId}">([^<]+)</pre>`))?.[1];
  if (!serialized) {
    assert.fail([
      `Fixture did not render its result; Chrome ran ${Date.now() - started} ms of its 45000.`,
      `Requests Vite never answered: ${unanswered(log()).join(", ") || "none"}`,
      `Chrome's log:\n${browser.log().slice(-4000)}`,
      `DOM:\n${stdout}`,
    ].join("\n"));
  }
  return serialized;
}

/** CDP's description of a key the keyboard sends: Enter, Escape, Tab, the arrows, Space, a digit or a letter. */
function keyEvent(key: string): { key: string; code: string; windowsVirtualKeyCode: number; text?: string | undefined } {
  const named: Record<string, [number, string?]> = {
    Enter: [13, "\r"], Escape: [27], Tab: [9], Backspace: [8], ArrowLeft: [37], ArrowUp: [38], ArrowRight: [39], ArrowDown: [40],
  };
  const found = named[key];
  if (found) return { key, code: key, windowsVirtualKeyCode: found[0], text: found[1] };
  if (key === " ") return { key, code: "Space", windowsVirtualKeyCode: 32, text: key };
  if (/^[0-9]$/.test(key)) return { key, code: `Digit${key}`, windowsVirtualKeyCode: key.charCodeAt(0), text: key };
  if (/^[a-z]$/i.test(key)) return { key, code: `Key${key.toUpperCase()}`, windowsVirtualKeyCode: key.toUpperCase().charCodeAt(0), text: key };
  throw new Error(`fixtureInput doesn't know the key ${JSON.stringify(key)}`);
}

/** How a live render's browser differs from a quick, idle one's. */
type LiveOptions = {
  /** It asks for reduced motion. */
  reducedMotion?: boolean;
  /** Each of the page's requests (its modules among them) is answered this much later. */
  latencyMs?: number;
  /** The request whose URL holds `url` (a module loaded on first use) is answered `ms` late. */
  slow?: { url: string; ms: number };
};

/**
 * Like `renderFixturePage`, for a fixture that measures motion: Chromium runs in real time (its
 * frames, animation callbacks and resize observers run as in the app, which virtual time stops),
 * in a 1280×976 window, and the result is read over the DevTools protocol once it shows.
 */
export async function renderFixtureLive(
  t: TestContext,
  page: string,
  resultId: string,
  timeoutMs = 60000,
  { reducedMotion = false, latencyMs = 0, slow }: LiveOptions = {},
): Promise<string> {
  const { url, scratch, log, stopFirst } = await serveFixtures(t);
  const deadline = Date.now() + timeoutMs;
  const browser = await openBrowser(t, scratch, stopFirst, ["--window-size=1280,976"], deadline);
  const { socket, call } = browser;
  // What a page can't do itself, done for it as a user's input would be (`fixtureInput`, in
  // fixtures/input.ts): a wheel turned over a point, the window resized, a key pressed (to
  // whatever has focus, so focus handling is what's tested), a click or a pointer move.
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data)) as { method?: string; params?: { name: string; payload: string } };
    if (message.method !== "Runtime.bindingCalled" || message.params?.name !== "fixtureInput") return;
    const asked = JSON.parse(message.params.payload) as { id: number } & FixtureInput;
    const done = () => call("Runtime.evaluate", { expression: `window.fixtureInputDone?.(${asked.id})` });
    const act = async () => {
      switch (asked.type) {
        case "wheel":
          return call("Input.dispatchMouseEvent", { type: "mouseWheel", x: asked.x, y: asked.y, deltaX: asked.deltaX ?? 0, deltaY: asked.deltaY ?? 0 });
        case "viewport":
          return call("Emulation.setDeviceMetricsOverride", { width: asked.width, height: asked.height, deviceScaleFactor: 1, mobile: false });
        case "key": {
          const key = keyEvent(asked.key);
          const held = asked.modifiers ?? [];
          const modifiers = held.reduce((bits, name) => bits | { Alt: 1, Control: 2, Meta: 4, Shift: 8 }[name], 0);
          // With ⌘ or Ctrl held a letter types nothing.
          const text = held.includes("Meta") || held.includes("Control") ? undefined : key.text;
          // A key that types (Enter's "\r", a digit) is a keyDown with its text, as a keyboard's.
          await call("Input.dispatchKeyEvent", { ...key, text, modifiers, type: text ? "keyDown" : "rawKeyDown" });
          return call("Input.dispatchKeyEvent", { ...key, modifiers, type: "keyUp", text: undefined });
        }
        case "type":
          // Each character as the keyboard sends it: a key down carrying its text, then up.
          for (const char of asked.text) {
            const code = /^[a-z]$/i.test(char) ? `Key${char.toUpperCase()}` : /^[0-9]$/.test(char) ? `Digit${char}` : "";
            await call("Input.dispatchKeyEvent", { type: "keyDown", key: char, code, text: char, unmodifiedText: char });
            await call("Input.dispatchKeyEvent", { type: "keyUp", key: char, code });
          }
          return undefined;
        case "click":
          await call("Input.dispatchMouseEvent", { type: "mouseMoved", x: asked.x, y: asked.y });
          await call("Input.dispatchMouseEvent", { type: "mousePressed", x: asked.x, y: asked.y, button: "left", buttons: 1, clickCount: 1 });
          return call("Input.dispatchMouseEvent", { type: "mouseReleased", x: asked.x, y: asked.y, button: "left", buttons: 0, clickCount: 1 });
        case "pointer":
          return call("Input.dispatchMouseEvent", { type: "mouseMoved", x: asked.x, y: asked.y });
      }
    };
    void act().then(done);
  });
  await call("Runtime.enable", {});
  await call("Runtime.addBinding", { name: "fixtureInput" });
  // The page acts as the focused one, as a window in front is: headless Chrome gives none the
  // window's focus, so a field's focus() would fire no focus event.
  await call("Emulation.setFocusEmulationEnabled", { enabled: true });
  if (reducedMotion) await call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  // A busy machine's server, slow to answer: every request, or one module.
  if (latencyMs > 0) {
    await call("Network.enable", {});
    await call("Network.emulateNetworkConditions", { offline: false, latency: latencyMs, downloadThroughput: -1, uploadThroughput: -1 });
  }
  if (slow) {
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as { method?: string; params?: { requestId: string } };
      if (message.method !== "Fetch.requestPaused") return;
      const { requestId } = message.params!;
      setTimeout(() => void call("Fetch.continueRequest", { requestId }).catch(() => {}), slow.ms);
    });
    await call("Fetch.enable", { patterns: [{ urlPattern: `*${slow.url}*` }] });
  }
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
    `Chrome's log:\n${browser.log().slice(-4000)}`,
  ].join("\n"));
}

