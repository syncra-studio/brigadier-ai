#!/usr/bin/env node
// Waits for what a tool can't cause: the user changing Spaces, or the display sleeping and waking. On its
// own window, on its strip. Quiet until something happens: then it reads what host, guest, capturePage (as
// the spike did) and the OS say, switches tab while away, and after each return requires a fresh paint in
// OS pixels and the same document in both tabs. Otherwise it only keeps a light heartbeat, with no capture.
// Stops at once if the dev app itself comes forward or asks for anything.
//   space-probe.mjs [hours]; read evidence/space-probe/summary.json
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

import { checkDir, eventsPath, evidenceDir } from "./paths.mjs";
import { capture } from "./instruments.mjs";
import { FIXTURES, control, guestExec, host, openShell, openTab, selectSession, selectTab, sleep, spaces } from "./lib.mjs";

const name = "spaceprobe";
const hours = Number(process.argv[2] ?? 24);
const folder = path.join(evidenceDir, "space-probe");
fs.mkdirSync(folder, { recursive: true });
const out = path.join(folder, "log.jsonl");
const powerOut = path.join(folder, "power.jsonl");
const write = (row) => fs.appendFileSync(out, `${JSON.stringify({ at: new Date().toISOString(), ...row })}\n`);
const started = new Date();
const startedIso = started.toISOString();
const summary = {
  running: true, startedAt: startedIso, endsAt: new Date(started.getTime() + hours * 3600000).toISOString(), lastHeartbeat: null, stoppedBecause: null,
  spaceChanges: 0, displayWakes: 0, displaySleeps: 0, returnsPassed: 0, returnsFailed: 0, returnsSkippedNotVisible: 0, failures: [],
  renderersGone: [], hostNavigations: 0, viteClientLines: [], devAppAlerts: [], outsideDialogs: 0, offStripOrFocused: [],
};
const saveSummary = () => fs.writeFileSync(path.join(folder, "summary.json"), JSON.stringify(summary, null, 1));

const fresh = async () => {
  const opened = await openShell(name, { level: "floating", allSpaces: false, bounds: (display) => ({ x: display.width - 38, y: 840 }) });
  return opened.windowId;
};
let windowId = await fresh();
await host(name, "localStorage.clear();1");
await control("closeWindow", { name });
windowId = await fresh();
const session = `session-${name}`;
await selectSession(name, session);
const a = await openTab(name, session, `${FIXTURES}/page/cc4400/probe-a`);
const b = await openTab(name, session, `${FIXTURES}/page/0044cc/probe-b`);
await selectTab(name, session, a.id);
await sleep(500);
const parkedAt = (await control("windowInfo", { name })).bounds;
const point = { x: 700, y: 400 };
let front = a;
const identity = async (tab) => ({
  wc: tab.wc,
  pid: (await control("guests")).find((guest) => guest.id === tab.wc)?.pid ?? null,
  timeOrigin: await guestExec(tab.wc, "performance.timeOrigin"),
  stamp: await guestExec(tab.wc, "window.__probeStamp ??= Math.random()"),
});
const before = new Map([[a, await identity(a)], [b, await identity(b)]]);

let shot = 0;
const read = async (why) => {
  const row = { why, active: spaces("list").active };
  row.hostFirst = await host(name, "document.visibilityState");
  row.guestFirst = await guestExec(front.wc, "document.visibilityState");
  row.capturePage = (await control("capturePageForRepro", { name, point }).catch((error) => ({ pixel: String(error.message).slice(0, 80) }))).pixel;
  row.hostAfterCapture = await host(name, "document.visibilityState");
  row.guestAfterCapture = await guestExec(front.wc, "document.visibilityState");
  row.hostCapturing = await control("hostCapturing", { name });
  const { file, image } = await capture(windowId, `probe-${String(shot++).padStart(3, "0")}-${why}`, folder);
  row.osPage = image.at(1400, 800);
  row.file = file;
  write(row);
  return row;
};

/** A colour chosen now reaching the OS's pixels, in this tab: only a live renderer can do that. */
const freshPaint = async (tab) => {
  const color = `f0${randomBytes(2).toString("hex")}`;
  const target = [0, 2, 4].map((at) => parseInt(color.slice(at, at + 2), 16));
  const startedAt = Date.now();
  await guestExec(tab.wc, `__paintNonce(${JSON.stringify(color)});1`);
  for (let tries = 0; tries < 40; tries++) {
    const { image, file } = await capture(windowId, "probe-fresh-latest", folder);
    const pixel = image.at(1400, 1540);
    if (target.every((value, at) => Math.abs(pixel[at] - value) < 60)) return { passed: true, ms: Date.now() - startedAt, color, pixel };
    if (tries === 39) fs.copyFileSync(file, path.join(folder, `probe-fresh-failed-${shot}.png`));
    await sleep(150);
  }
  return { passed: false, ms: Date.now() - startedAt, color };
};

let owed = null;
/** After a return: both tabs live and the same documents, or a recorded failure. Not retried if it fails. */
const returnCheck = async (why) => {
  const hostState = await host(name, "document.visibilityState");
  if (hostState !== "visible") {
    summary.returnsSkippedNotVisible++;
    write({ why: `${why}: window not visible yet, checks when it is`, hostState });
    owed = why;
    return;
  }
  owed = null;
  const row = { why: `return check: ${why}`, tabs: [] };
  let passed = true;
  for (const tab of [front, front === a ? b : a]) {
    await selectTab(name, session, tab.id);
    await sleep(500);
    const paint = await freshPaint(tab);
    const now = await identity(tab);
    const same = JSON.stringify(now) === JSON.stringify(before.get(tab));
    row.tabs.push({ url: await guestExec(tab.wc, "location.pathname"), paint, same, now });
    passed &&= paint.passed && same;
  }
  await selectTab(name, session, front.id);
  row.passed = passed;
  const { file } = await capture(windowId, `probe-${String(shot++).padStart(3, "0")}-returned`, folder);
  row.file = file;
  write(row);
  if (passed) summary.returnsPassed++;
  else {
    summary.returnsFailed++;
    summary.failures.push({ at: new Date().toISOString(), why, tabs: row.tabs, file });
  }
};

// The oracle against itself, before trusting anything it says later.
await returnCheck("self-test at start");
if (summary.returnsPassed !== 1) {
  summary.running = false;
  summary.stoppedBecause = "the self-test failed";
  saveSummary();
  process.exit(1);
}
summary.returnsPassed = 0;
const home = spaces("list").active;
write({ why: "start", home, windowId, parkedAt, hours });
await read("start");

const powerSeen = new Set();
const readPower = () => {
  let text = "";
  try {
    text = execFileSync("/bin/sh", ["-c", "/usr/bin/pmset -g log | /usr/bin/grep -E 'Display is turned|^[0-9: +-]+ (Sleep|Wake|DarkWake) '"], { maxBuffer: 1 << 26 }).toString();
  } catch { return []; }
  const fresh = [];
  for (const line of text.split("\n")) {
    const match = /^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d) ([+-]\d{4}) (\S+)\s+(.*)$/.exec(line);
    if (!match || powerSeen.has(line)) continue;
    powerSeen.add(line);
    const at = new Date(`${match[1].replace(" ", "T")}${match[2].slice(0, 3)}:${match[2].slice(3)}`);
    if (at < started) continue;
    const row = { at: at.toISOString(), type: match[3], message: match[4].trim().slice(0, 120) };
    fs.appendFileSync(powerOut, `${JSON.stringify(row)}\n`);
    fresh.push(row);
  }
  return fresh;
};
readPower();

const alertsFile = path.join(checkDir, "ALERTS.jsonl");
let alertsRead = fs.existsSync(alertsFile) ? fs.readFileSync(alertsFile, "utf8").split("\n").filter(Boolean).length : 0;
const stop = async (because) => {
  summary.running = false;
  summary.stoppedBecause = because;
  saveSummary();
  try { await control("closeWindow", { name }); } catch { /* it may be what went */ }
  process.exit(because === "time is up" ? 0 : 1);
};

const end = started.getTime() + hours * 3600000;
let last = home;
let nextPower = Date.now() + 120000;
let nextHeartbeat = Date.now() + 600000;
let eventsRead = fs.statSync(eventsPath).size;
saveSummary();
while (Date.now() < end) {
  try {
    const { active } = spaces("list");
    if (active !== last) {
      last = active;
      summary.spaceChanges++;
      write({ why: "space-changed", active, atHome: active === home });
      if (active === home) {
        await read("just-back");
        await returnCheck("Space change");
      } else {
        await read("just-left");
        // As the spike did while away: another tab, then read again.
        front = front === a ? b : a;
        await selectTab(name, session, front.id);
        await sleep(800);
        await read("away-switched");
        await sleep(4000);
        await read("away+5s");
      }
      saveSummary();
    }
    if (Date.now() > nextPower) {
      nextPower = Date.now() + 120000;
      for (const row of readPower()) {
        if (/Display is turned off/.test(row.message)) summary.displaySleeps++;
        if (/Display is turned on/.test(row.message)) {
          summary.displayWakes++;
          if (spaces("list").active === home) {
            await read("display-woke");
            await returnCheck(`display woke at ${row.at}`);
          } else owed = `display woke at ${row.at}`;
        }
      }
      saveSummary();
    }
    if (owed && spaces("list").active === home && (await host(name, "document.visibilityState")) === "visible") await returnCheck(owed);
    // Only the watcher's and the app's own files, and only what's new in them.
    const rows = fs.existsSync(alertsFile) ? fs.readFileSync(alertsFile, "utf8").split("\n").filter(Boolean) : [];
    for (const line of rows.slice(alertsRead)) {
      const row = JSON.parse(line);
      if (row.isWatched === true || (row.kind === "active" && row.active === true) || row.kind === "log-tcc-prompt") summary.devAppAlerts.push(row);
      else if (row.kind !== "dev-window-on-user-screen") summary.outsideDialogs++;
    }
    alertsRead = rows.length;
    if (summary.devAppAlerts.length) await stop("the dev app came forward or asked for something: see devAppAlerts");
    const size = fs.statSync(eventsPath).size;
    if (size !== eventsRead) {
      const descriptor = fs.openSync(eventsPath, "r");
      const buffer = Buffer.alloc(size - eventsRead);
      fs.readSync(descriptor, buffer, 0, buffer.length, eventsRead);
      fs.closeSync(descriptor);
      eventsRead = size;
      for (const line of buffer.toString().split("\n").filter(Boolean)) {
        const row = JSON.parse(line);
        if (row.kind === "render-process-gone" || row.kind === "child-process-gone") summary.renderersGone.push(row);
        if (row.kind === "host-navigation" && row.url?.includes(`bcw=${name}`)) summary.hostNavigations++;
        if (row.kind === "vite-client" && !/connecting|connected\./.test(row.message)) summary.viteClientLines.push(row);
      }
    }
    if (Date.now() > nextHeartbeat) {
      nextHeartbeat = Date.now() + 600000;
      const info = await control("windowInfo", { name });
      const placed = info.bounds.x === parkedAt.x && info.bounds.y === parkedAt.y;
      if (!placed || info.focused) summary.offStripOrFocused.push({ at: new Date().toISOString(), bounds: info.bounds, focused: info.focused });
      const watcher = execFileSync("/bin/sh", ["-c", "/usr/bin/pgrep -f 'browser-check/bin/watcher' | /usr/bin/wc -l; /usr/bin/pgrep -f '/usr/bin/log stream' | /usr/bin/wc -l"]).toString().trim().split(/\s+/);
      summary.watcherProcesses = Number(watcher[0]);
      summary.logStreams = Number(watcher[1]);
      summary.lastHeartbeat = new Date().toISOString();
      saveSummary();
    }
  } catch (error) {
    const text = String(error?.message ?? error);
    write({ why: "error", error: text.slice(0, 300) });
    if (/ECONNREFUSED|ENOENT|the app exited|no answer/.test(text)) await stop(`the instance went away: ${text.slice(0, 120)}`);
  }
  await sleep(2000);
}
await stop("time is up");
