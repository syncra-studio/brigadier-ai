#!/usr/bin/env node
// The gate for section 1: "tabs open, switch, sit idle for 30 minutes and come back, with no blank tab."
//
//   gate.mjs idle [minutes] [cases...]   uninterrupted idle, then return. Nothing touches the tabs meanwhile.
//   gate.mjs stress [minutes]            tab switching, separately.
//
// A case passes only if, after the return, every tab shows its own content in OS pixels, responds with
// a fresh paint (a colour chosen after the return, so stale pixels can't pass), and is the same guest
// and the same document: same webContents, renderer, time origin, and a stamp set before the idle, with
// no navigation or new page in between. A capture that fails is a failure. All input here is scripted.
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

import { checkDir, eventsPath, evidenceDir } from "./paths.mjs";
import { capture, osWindows } from "./instruments.mjs";
import { FIXTURES, control, guestExec, host, layerState, openShell, openTab, selectSession, selectTab, sleep, spaces, startCover } from "./lib.mjs";
import { distinctColors, near } from "./png.mjs";

const [mode = "idle", minutesArg, ...caseArgs] = process.argv.slice(2);
// The dev server's client reloads the page when its socket reconnects, which would destroy every tab.
if (process.env.BRIGADIER_NO_HMR !== "1") throw new Error("Start the instance with BRIGADIER_NO_HMR=1 for the gate, and run this with it too");
const minutes = Number(minutesArg ?? 30);
const stamp = new Date().toISOString().replace(/[:.]/g, "-");
const folder = path.join(evidenceDir, `gate-${mode}-${stamp}`);
fs.mkdirSync(folder, { recursive: true });
const log = (...parts) => console.log(new Date().toISOString(), ...parts);
const alertsFile = path.join(checkDir, "ALERTS.jsonl");
const alertCount = () => (fs.existsSync(alertsFile) ? fs.readFileSync(alertsFile, "utf8").split("\n").filter(Boolean).length : 0);
const alertRows = () => (fs.existsSync(alertsFile) ? fs.readFileSync(alertsFile, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)) : []);
/** The app's own doing stops a run at once; a dialog from something else is listed for a person to read. */
const isFromDevApp = (row) => row.isWatched === true || (row.kind === "active" && row.active === true) || row.kind === "log-tcc-prompt";
const events = () => fs.readFileSync(eventsPath, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));

// Keeps the Mac awake for the run, not the display: it's the same switch a user's other tools use.
const awake = spawn("/usr/bin/caffeinate", ["-i", "-w", String(process.pid)], { stdio: "ignore", detached: true });
awake.unref();

const REAL_SITES = [
  ["wikipedia", "https://en.wikipedia.org/wiki/Web_browser"],
  ["hackernews", "https://news.ycombinator.com/"],
];
const PAGE_RECT = { x: 620, y: 200, width: 1900, height: 1200 };
const COLORS = { shown: "336699", behind: "993366", minimised: "669933", space: "996633", hidden: "339966", stress: "663399", spaceprobe: "cc4400" };
const hex = () => randomBytes(3).toString("hex");
const toRgb = (color) => [0, 2, 4].map((at) => parseInt(color.slice(at, at + 2), 16));

async function setUp(name, index, options) {
  const { windowId } = await openShell(name, { index, ...options });
  const session = `session-${name}`;
  await selectSession(name, session);
  const tabs = [];
  const local = await openTab(name, session, `${FIXTURES}/page/${COLORS[name]}/${name}`);
  tabs.push({ label: "local", ...local });
  for (const [label, url] of REAL_SITES) {
    const tab = await openTab(name, session, url);
    tabs.push({ label, ...tab });
    await sleep(4000);
  }
  for (const tab of tabs) {
    tab.identity = await identityOf(name, tab);
    await guestExec(tab.wc, `window.__gateStamp=${JSON.stringify(`${name}-${tab.label}`)};1`);
  }
  await selectTab(name, session, local.id);
  await sleep(500);
  return { name, windowId, session, tabs };
}

async function identityOf(name, tab) {
  const guest = (await control("guests")).find((each) => each.id === tab.wc);
  const page = (await layerState(name)).pages.find((each) => each.id === tab.id);
  return {
    wc: tab.wc,
    pid: guest?.pid ?? null,
    timeOrigin: await guestExec(tab.wc, "performance.timeOrigin"),
    navigation: await guestExec(tab.wc, "navigation.currentEntry?.id ?? null"),
    url: guest?.url ?? null,
    index: page?.indexInLayer ?? null,
  };
}

/** Each tab in turn: shows it, captures it, remembers what it looks like. */
async function photographAll(run, label) {
  for (const tab of run.tabs) {
    await selectTab(run.name, run.session, tab.id);
    await sleep(tab.label === "local" ? 500 : 900);
    const { file, image } = await capture(run.windowId, `${run.name}-${tab.label}-${label}`, folder);
    tab[label] = { file, grid: gridOf(image), distinct: distinctColors(image, PAGE_RECT, 16) };
  }
  await selectTab(run.name, run.session, run.tabs[0].id);
  await sleep(300);
}

function gridOf(image) {
  const grid = [];
  for (let y = PAGE_RECT.y; y < PAGE_RECT.y + PAGE_RECT.height; y += 60) {
    for (let x = PAGE_RECT.x; x < PAGE_RECT.x + PAGE_RECT.width; x += 60) grid.push(image.at(x, y));
  }
  return grid;
}
const sameFraction = (a, b) => a.filter((pixel, at) => near(pixel, b[at], 24)).length / a.length;

/** A colour nobody has seen, painted in the guest, must reach the OS's pixels: the proof it's live. */
async function freshPaint(run, tab) {
  const color = hex().replace(/^(..)/, "f0");
  const started = Date.now();
  await guestExec(tab.wc, `(()=>{let e=document.getElementById("__gate");if(!e){e=document.createElement("div");e.id="__gate";e.style.cssText="position:fixed;right:0;top:0;width:300px;height:200px;z-index:2147483647";document.documentElement.append(e)}e.style.background="#${color}";return 1})()`);
  const target = toRgb(color);
  for (let tries = 0; tries < 40; tries++) {
    const { image, file } = await capture(run.windowId, `${run.name}-${tab.label}-fresh`, folder);
    const page = (await layerState(run.name)).pages.find((each) => each.id === tab.id).rect;
    const pixel = image.at(Math.round((page.x + page.width - 100) * 2), Math.round((page.y + 60) * 2));
    // The screen's colour profile moves values a little; a colour chosen now can't be there by chance.
    if (Math.abs(pixel[0] - target[0]) < 60 && Math.abs(pixel[1] - target[1]) < 60 && Math.abs(pixel[2] - target[2]) < 60) {
      await guestExec(tab.wc, `document.getElementById("__gate")?.remove();1`);
      return { passed: true, ms: Date.now() - started, color, pixel, file };
    }
    await sleep(100);
  }
  return { passed: false, ms: Date.now() - started, color };
}

async function judge(run, sinceIso, elapsedMs) {
  const rows = [];
  const allEvents = events();
  const since = allEvents.filter((event) => event.at >= sinceIso);
  for (const tab of run.tabs) {
    await selectTab(run.name, run.session, tab.id);
    await sleep(tab === run.tabs[0] ? 100 : 700);
    const after = await capture(run.windowId, `${run.name}-${tab.label}-after`, folder);
    const grid = gridOf(after.image);
    const distinct = distinctColors(after.image, PAGE_RECT, 16);
    const fresh = await freshPaint(run, tab);
    const identity = await identityOf(run.name, tab);
    const stampStill = await guestExec(tab.wc, "window.__gateStamp");
    const navigations = since.filter((event) => event.kind === "guest-navigation" && event.webContents === tab.wc && !event.inPlace);
    const gone = since.filter((event) => event.kind === "render-process-gone" && event.webContents === tab.wc);
    const similar = sameFraction(grid, tab.before.grid);
    const notBlank = tab.label === "local" ? near(after.image.at(1500, 900), tab.before.grid[Math.floor(tab.before.grid.length / 2)], 40) : distinct >= 5;
    const sameIdentity = ["wc", "pid", "timeOrigin", "navigation", "index"].every((key) => identity[key] === tab.identity[key]);
    const row = {
      case: run.name,
      tab: tab.label,
      url: identity.url,
      identifiablePixels: notBlank && similar > 0.6,
      similarToBefore: Number(similar.toFixed(3)),
      distinctColors: distinct,
      freshPaint: fresh.passed,
      freshMs: fresh.ms,
      sameGuestAndDocument: sameIdentity && stampStill === `${run.name}-${tab.label}`,
      identityBefore: tab.identity,
      identityAfter: identity,
      navigationsDuringIdle: navigations.length,
      rendererExits: gone,
      elapsedMs,
      before: tab.before.file,
      after: after.file,
    };
    row.passed = row.identifiablePixels && row.freshPaint && row.sameGuestAndDocument && navigations.length === 0 && gone.length === 0;
    rows.push(row);
    log(row.passed ? "PASS" : "FAIL", run.name, tab.label, `similar=${row.similarToBefore} fresh=${row.freshPaint}/${row.freshMs}ms same=${row.sameGuestAndDocument} navs=${navigations.length} exits=${gone.length}`);
  }
  await selectTab(run.name, run.session, run.tabs[0].id);
  return rows;
}

/** What the log says asked for a dialog: an admin prompt's client, and processes that start to draw one. */
function dialogSources(sinceIso) {
  const start = new Date(sinceIso).toLocaleString("sv").slice(0, 19);
  const rows = execFileSync("/usr/bin/log", ["show", "--start", start, "--style", "compact", "--predicate", 'process == "authd" AND eventMessage CONTAINS "Succeeded authorizing"'], { maxBuffer: 1 << 26 }).toString().split("\n");
  const agents = execFileSync("/bin/ps", ["-axo", "lstart,comm"]).toString().split("\n").filter((line) => /IOUIAgent|CoreServicesUIAgent|SecurityAgent/.test(line));
  return [...rows.filter((line) => line.includes("client")).slice(-5).map((line) => line.slice(0, 220)), ...agents.map((line) => line.trim().slice(0, 160))];
}

/** Each case's own strip of the screen, so no case's state touches another's. */
const strip = {
  behind: (display) => ({ x: display.width - 14, y: 33 }),
  minimised: (display) => ({ x: display.width - 14, y: 840 }),
  space: (display) => ({ x: display.width - 62, y: 840 }),
  shown: (display) => ({ x: display.width - 14, y: 840 }),
  hidden: (display) => ({ x: display.width - 14, y: 840 }),
};

/** Cases: what state a window sits in for the idle, how it comes back, and what it should report. */
const CASES = {
  shown: { options: { level: "pop-up-menu", allSpaces: true }, enter: async () => {}, leave: async () => {}, expect: "visible" },
  behind: {
    options: { level: "floating", allSpaces: true },
    enter: async (run, context) => {
      const { display } = await control("info");
      // A plain window at the normal level, under a real window of another process.
      await control("setLevel", { name: run.name, level: "normal" });
      context.cover = startCover(display.width - 30, display.height - 833, 30, 800);
    },
    leave: async (run, context) => {
      context.cover.kill();
      await control("setLevel", { name: run.name, level: "floating" });
    },
    expect: "hidden",
  },
  minimised: {
    options: { level: "floating", allSpaces: true },
    enter: async (run) => control("minimize", { name: run.name }),
    leave: async (run) => control("restore", { name: run.name }),
    expect: "hidden",
  },
  // A tool can't move a window to another Space (the private calls are refused), so this window stays on
  // the Space it was shown on, and the run records whether the user left it. Unexercised if they didn't.
  space: { options: { level: "floating", allSpaces: false }, enter: async () => {}, leave: async () => {}, expect: "either" },
  hidden: {
    options: { level: "floating", allSpaces: true },
    enter: async () => control("hideApp"),
    // Un-hiding moves the window: wait for it to be back in place before judging it.
    leave: async () => { await control("showApp"); await sleep(600); },
    expect: "hidden",
  },
};

async function runIdle(names) {
  const runs = [];
  const context = Object.fromEntries(names.map((name) => [name, {}]));
  for (const name of names) {
    log("setting up", name);
    const run = await setUp(name, 0, { ...CASES[name].options, bounds: strip[name] });
    await photographAll(run, "before");
    await CASES[name].enter(run, context[name]);
    run.entered = Date.now();
    run.enteredIso = new Date(run.entered).toISOString();
    runs.push(run);
    if (name === "space") {
      // The Space probe sits above this window and below "shown". It's the only window read during the idle.
      const probeLog = path.join(evidenceDir, "space-probe/log.jsonl");
      const sizeBefore = fs.existsSync(probeLog) ? fs.statSync(probeLog).size : 0;
      spawn(process.execPath, [new URL("./space-probe.mjs", import.meta.url).pathname, String(minutes + 8)], { stdio: "ignore", detached: true, env: process.env }).unref();
      for (let tries = 0; tries < 120 && !(fs.existsSync(probeLog) && fs.statSync(probeLog).size > sizeBefore); tries++) await sleep(500);
      await sleep(2000);
    }
  }
  await sleep(2500);
  for (const run of runs) run.stateAtStart = await host(run.name, "document.visibilityState");
  // From here nothing speaks to the case windows. Alerts count from here too: minimising animates over
  // the screen, and those transitions are reported apart from the idle.
  const transitionAlerts = alertCount();
  const pidsAtStart = new Map((await control("metrics")).map((m) => [m.pid, m.type]));
  const start = new Date(Math.max(...runs.map((run) => run.entered)));
  const startIso = start.toISOString();
  log(`idle starts, ${minutes} min, untouched:`, runs.map((run) => `${run.name}=${run.stateAtStart}`).join(" "));

  // Below is only the OS's view: process list, window list and active Space, every 10 s.
  const timeline = [];
  const appPid = (await control("info")).pid;
  let stopped = null;
  let offStrip = 0;
  const { display } = await control("info");
  const startSpace = spaces("list").active;
  while (Date.now() - start.getTime() < minutes * 60000) {
    await sleep(10000);
    const alive = execFileSync("/bin/ps", ["-axo", "pid="]).toString().split("\n").map(Number);
    const missing = [...pidsAtStart.keys()].filter((pid) => !alive.includes(pid));
    const active = spaces("list").active;
    const windows = (await osWindows(appPid)).filter((each) => runs.some((run) => run.windowId === each.id)).map((each) => [each.id, each.onscreen]);
    timeline.push({ at: new Date().toISOString(), missing, active, windows });
    fs.writeFileSync(path.join(folder, "timeline.json"), JSON.stringify(timeline));
    // A Space change slides a window's reported frame for a moment: only one that stays off its strip counts.
    const onScreen = (await osWindows(appPid)).filter((each) => runs.some((run) => run.windowId === each.id) && each.onscreen && each.bounds.X < display.width - 70);
    offStrip = onScreen.length ? offStrip + 1 : 0;
    const fromDev = [...alertRows().slice(transitionAlerts).filter(isFromDevApp), ...(offStrip >= 2 ? [{ kind: "window stayed on the user's screen", windows: onScreen }] : [])];
    if (fromDev.length) {
      stopped = fromDev[0];
      log("STOP: the dev app's own alert", JSON.stringify(fromDev[0]));
      break;
    }
    if (missing.length) log("process gone during idle", missing.map((pid) => `${pid} ${pidsAtStart.get(pid)}`).join(", "));
  }
  const idleRows = alertRows().slice(transitionAlerts);
  const idleAlerts = idleRows.filter(isFromDevApp).length;
  const outsideDialogs = idleRows.filter((row) => !isFromDevApp(row) && row.kind !== "dev-window-on-user-screen");
  const idleEnd = Date.now();
  log("idle ends", Math.round((idleEnd - start.getTime()) / 1000), "s");
  // Every renderer exit, with its reason, clean exits included.
  const exits = events().filter((event) => event.at >= runs[0].enteredIso && (event.kind === "render-process-gone" || event.kind === "child-process-gone"));
  for (const run of runs) await CASES[run.name].leave(run, context[run.name]);
  await sleep(150);
  const rows = [];
  for (const run of runs) {
    const stateAfter = await host(run.name, "document.visibilityState");
    const caseRows = await judge(run, run.enteredIso, idleEnd - run.entered);
    const expected = CASES[run.name].expect;
    const stateOk = expected === "either" || run.stateAtStart === expected;
    rows.push(...caseRows.map((row) => ({ ...row, stateAtStart: run.stateAtStart, stateAfter, stateOk, passed: row.passed && stateOk })));
  }
  const spaceChanges = timeline.filter((row, at) => at > 0 && row.active !== timeline[at - 1].active).length;
  const report = {
    mode,
    minutes,
    start: startIso,
    elapsedMs: idleEnd - start.getTime(),
    stopped,
    startSpace,
    spaceChanges,
    windowOffScreenSamples: Object.fromEntries(runs.map((run) => [run.name, timeline.filter((row) => row.windows.some(([id, on]) => id === run.windowId && !on)).length])),
    samples: timeline.length,
    exits,
    processesGone: [...new Set(timeline.flatMap((row) => row.missing))].map((pid) => ({ pid, type: pidsAtStart.get(pid) })),
    idleAlerts,
    outsideDialogs,
    outsideDialogSources: outsideDialogs.length ? dialogSources(startIso) : [],
    alertsOnReturnAndSetup: alertCount() - idleRows.length,
    layers: Object.fromEntries(await Promise.all(runs.map(async (run) => [run.name, (await layerState(run.name)).counters]))),
    passed: !stopped && rows.every((row) => row.passed) && exits.length === 0 && idleAlerts === 0,
    rows,
  };
  fs.writeFileSync(path.join(folder, "report.json"), JSON.stringify(report, null, 1));
  log(report.passed ? "GATE PASSED" : "GATE FAILED", path.join(folder, "report.json"), `exits=${exits.length} idleAlerts=${idleAlerts} spaceChanges=${spaceChanges}`);
  return report;
}

async function runStress() {
  const run = await setUp("stress", 0, { level: "floating", allSpaces: true, bounds: (display) => ({ x: display.width - 14, y: 840 }) });
  await photographAll(run, "before");
  const start = Date.now();
  const startIso = new Date(start).toISOString();
  const alertsBefore = alertCount();
  let switches = 0;
  let blanks = 0;
  const deadline = start + minutes * 60000;
  while (Date.now() < deadline) {
    const tab = run.tabs[switches % run.tabs.length];
    await selectTab(run.name, run.session, tab.id);
    switches++;
    // Mostly as fast as possible; a settle and a real capture every so often.
    if (switches % 25 === 0) {
      await sleep(250);
      const { image, file } = await capture(run.windowId, "stress-latest", folder);
      if (sameFraction(gridOf(image), tab.before.grid) < 0.5) {
        blanks++;
        fs.copyFileSync(file, path.join(folder, `stress-unlike-${switches}.png`));
        log("unlike its tab at switch", switches, tab.label);
      }
    } else await sleep(30);
    if (alertRows().slice(alertsBefore).some(isFromDevApp)) break;
  }
  const elapsedMs = Date.now() - start;
  const rows = await judge(run, startIso, elapsedMs);
  const stressRows = alertRows().slice(alertsBefore);
  const report = { mode, minutes, elapsedMs, switches, blanks, alerts: stressRows.filter(isFromDevApp).length, outsideOrOsEvents: stressRows.filter((row) => !isFromDevApp(row)), rows, counters: (await layerState(run.name)).counters, exits: events().filter((event) => event.at >= startIso && event.kind.endsWith("process-gone")) };
  report.passed = blanks === 0 && report.exits.length === 0 && rows.every((row) => row.passed) && report.alerts === 0;
  fs.writeFileSync(path.join(folder, "report.json"), JSON.stringify(report, null, 1));
  log(report.passed ? "STRESS PASSED" : "STRESS FAILED", path.join(folder, "report.json"), `switches=${switches} blanks=${blanks}`);
  return report;
}

const report = mode === "stress" ? await runStress() : await runIdle(caseArgs.length ? caseArgs : ["behind", "minimised", "space", "shown"]);
process.exit(report.passed ? 0 : 1);
