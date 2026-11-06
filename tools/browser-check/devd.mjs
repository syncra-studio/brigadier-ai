#!/usr/bin/env node
// Keeps one Brigadier Dev alive: the front end's dev server, the app opened without activation under
// a system-side watcher, and the debugging pipe, which it serves to bc.mjs over a socket. The pipe is
// fds 3 and 4, which only a direct spawn can pass; spawn-disclaimed makes the app its own responsible
// process, as a Dock launch would. Chromium quits when the pipe closes, so the app can't outlive this.
import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";

import { addonPath, appBinary, binDir, checkDir, daemonDataDir, daemonHome, dataDir, desktopDir, electronDir, eventsPath, evidenceDir, sidecarDir, socketPath, vitePort } from "./paths.mjs";
import { startFixtures } from "./fixture-pages.mjs";
import { startWatch } from "./watch.mjs";

fs.mkdirSync(checkDir, { recursive: true });
fs.mkdirSync(dataDir, { recursive: true });
fs.mkdirSync(daemonDataDir, { recursive: true });
fs.mkdirSync(daemonHome, { recursive: true });
fs.mkdirSync(evidenceDir, { recursive: true });
const log = (...parts) => console.log(new Date().toISOString(), ...parts);

let app = null;
let nextId = 1;
const pending = new Map();
let write = null;
let controlSession = null;
const hostSessions = new Map();
let stopWatch = null;

function send(method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject, method });
    write(`${JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })}\0`);
  });
}

function onMessage(raw) {
  const message = JSON.parse(raw);
  if (message.id === undefined) return;
  const waiter = pending.get(message.id);
  if (!waiter) return;
  pending.delete(message.id);
  if (message.error) waiter.reject(new Error(`${waiter.method}: ${message.error.message}`));
  else waiter.resolve(message.result);
}

async function evaluate(sessionId, expression) {
  const { result, exceptionDetails } = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, sessionId);
  if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
  return result.value;
}

async function attach(match, wait = 30000) {
  const start = Date.now();
  for (;;) {
    const { targetInfos } = await send("Target.getTargets");
    const target = targetInfos.find(match);
    if (target) {
      const { sessionId } = await send("Target.attachToTarget", { targetId: target.targetId, flatten: true });
      return sessionId;
    }
    if (Date.now() - start > wait) throw new Error("no such target");
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

async function launch() {
  fs.appendFileSync(eventsPath, "");
  const out = fs.openSync(path.join(checkDir, "app.log"), "a");
  const env = {
    ...process.env,
    BRIGADIER_DEV: "1",
    BRIGADIER_UNFOCUSED: process.env.BRIGADIER_FOCUSED === "1" ? "0" : "1",
    BRIGADIER_DEV_DATA: dataDir,
    BRIGADIER_EVENTS: eventsPath,
    BRIGADIER_RENDERER_URL: `http://127.0.0.1:${vitePort}/`,
    BRIGADIER_ADDON: addonPath,
    BRIGADIER_CONTROL_HTML: path.join(electronDir, "control.html"),
    BRIGADIER_DEV_DAEMON_DATA: daemonDataDir,
    BRIGADIER_DEV_SIDECARS: sidecarDir,
    // Off the user's CLI sign-ins unless a check says otherwise.
    BRIGADIER_DEV_DAEMON_HOME: process.env.BRIGADIER_DEV_DAEMON_HOME ?? daemonHome,
  };
  stopWatch = await startWatch({ checkDir });
  app = spawn(path.join(binDir, "spawn-disclaimed"), [appBinary, "--remote-debugging-pipe"], { env, stdio: ["ignore", out, out, "pipe", "pipe"] });
  stopWatch.setPid(app.pid);
  log("app pid", app.pid);
  write = (text) => app.stdio[3].write(text);
  let buffer = "";
  app.stdio[4].on("data", (chunk) => {
    buffer += chunk.toString("utf8");
    let at;
    while ((at = buffer.indexOf("\0")) >= 0) {
      onMessage(buffer.slice(0, at));
      buffer = buffer.slice(at + 1);
    }
  });
  const pid = app.pid;
  app.on("exit", (code, signal) => {
    log("app exited", pid, code, signal);
    if (app?.pid === pid) app = null;
    for (const [, waiter] of pending) waiter.reject(new Error("the app exited"));
    pending.clear();
    controlSession = null;
    hostSessions.clear();
  });
  const session = await attach((t) => t.url.endsWith("control.html"));
  // Attached can be before its preload has run.
  while (!(await evaluate(session, "Boolean(window.brigadierShell)"))) await new Promise((resolve) => setTimeout(resolve, 50));
  controlSession = session;
}

async function hostSession(name) {
  const existing = hostSessions.get(name);
  if (existing) return existing;
  const session = await attach((t) => t.type === "page" && new URL(t.url).searchParams.get("bcw") === name);
  hostSessions.set(name, session);
  return session;
}

async function control(op, args) {
  return evaluate(controlSession, `window.brigadierShell.devControl(${JSON.stringify(op)}, ${JSON.stringify(args ?? {})})`);
}

// The front end's server.
const vite = spawn(path.join(desktopDir, "node_modules/.bin/vite"), ["--config", "electron/vite.config.ts"], { cwd: desktopDir, stdio: ["ignore", fs.openSync(path.join(checkDir, "vite.log"), "a"), fs.openSync(path.join(checkDir, "vite.log"), "a")] });
for (let tries = 0; ; tries++) {
  try {
    const response = await fetch(`http://127.0.0.1:${vitePort}/`);
    if (response.ok) break;
  } catch { /* starting */ }
  if (tries > 120) throw new Error("the dev server did not start");
  await new Promise((resolve) => setTimeout(resolve, 500));
}
log("vite up");
await startFixtures();
await launch();

async function shutdown(code = 0) {
  try { if (controlSession) await control("quit"); } catch { /* already gone */ }
  await new Promise((resolve) => setTimeout(resolve, 1500));
  if (app) app.kill("SIGTERM");
  vite.kill("SIGTERM");
  stopWatch?.stop();
  fs.rmSync(socketPath, { force: true });
  process.exit(code);
}

async function handle(message) {
  switch (message.op) {
    case "status":
      return { pid: app?.pid ?? null, controlReady: controlSession !== null, vite: vite.pid, devd: process.pid };
    case "targets":
      return (await send("Target.getTargets")).targetInfos.map(({ type, url, title, targetId }) => ({ type, url, title, targetId }));
    case "control":
      return control(message.name, message.args);
    case "host": {
      try {
        return await evaluate(await hostSession(message.window), message.js);
      } catch (error) {
        if (!String(error.message).includes("Session with given id not found")) throw error;
        // The window was replaced: its page has another session now.
        hostSessions.delete(message.window);
        return evaluate(await hostSession(message.window), message.js);
      }
    }
    case "restart": {
      // One real relaunch, for what only a restart can show. `kill` is the crash version.
      const old = app.pid;
      await control(message.signal === "kill" ? "kill" : "quit").catch(() => {});
      for (let tries = 0; app?.pid === old && tries < 100; tries++) await new Promise((resolve) => setTimeout(resolve, 100));
      if (app?.pid === old) app.kill("SIGTERM");
      stopWatch?.stop();
      await launch();
      return { old, pid: app.pid };
    }
    case "down":
      setTimeout(() => void shutdown(), 50);
      return true;
    default:
      throw new Error(`unknown op ${message.op}`);
  }
}

fs.rmSync(socketPath, { force: true });
net.createServer({ allowHalfOpen: true }, (socket) => {
  let data = "";
  socket.on("data", (chunk) => { data += chunk; });
  socket.on("end", async () => {
    let reply;
    try {
      reply = { result: (await handle(JSON.parse(data))) ?? null };
    } catch (error) {
      reply = { error: String(error?.message ?? error) };
    }
    socket.end(JSON.stringify(reply));
  });
}).listen(socketPath);
log("devd ready");
process.on("SIGTERM", () => void shutdown());
