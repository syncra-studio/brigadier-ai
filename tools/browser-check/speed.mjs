#!/usr/bin/env node
// How fast the browser is, on the real front end, unfocused. Times come from OS frames, recorded at 120 fps
// through ScreenCaptureKit, against the machine's own clock: a time runs from the moment this script sends
// the action (process.hrtime and the frames' presentation times are both on it, which the run checks) to
// the first frame showing the result. A marker painted by the app can't do this: it's painted by the same
// busy thread as the work, and would start the clock late. So a time includes the trip over the debugging
// pipe (measured and reported), the app, the guest, the compositor and the window server, and resolves to a
// frame, about 8 ms. Input is scripted and the app is the dev build (React's development mode, a dev
// server), so these are upper bounds, except that a real pointer's route isn't measured at all.
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";

import { binDir, evidenceDir } from "./paths.mjs";
import { capture, framesOf } from "./instruments.mjs";
import { FIXTURES, control, guestExec, host, layerState, openShell, openTab, selectSession, selectTab, sleep, startCover } from "./lib.mjs";

const folder = path.join(evidenceDir, "speed");
fs.mkdirSync(folder, { recursive: true });
// Each part can run alone: speed.mjs [newtab] [switch] [address] [input] [idle] [cpu]; none means all.
const parts = process.argv.slice(2);
const want = (part) => parts.length === 0 || parts.includes(part);
const resultsFile = path.join(folder, "results.json");
const results = parts.length && fs.existsSync(resultsFile) ? JSON.parse(fs.readFileSync(resultsFile, "utf8")) : { at: new Date().toISOString(), timings: {}, idle: [] };
const save = () => fs.writeFileSync(path.join(folder, "results.json"), JSON.stringify(results, null, 1));
const log = (...parts) => console.log(new Date().toISOString().slice(11, 19), ...parts);

const name = "speed";
const session = `session-${name}`;
const { windowId } = await openShell(name);
await selectSession(name, session);
await sleep(500);

const REGION = { x: 700, y: 450, width: 16, height: 16 };
const HEADING = { x: 360, y: 190, width: 160, height: 40 };
const near = (a, b, t) => Math.abs(a[0] - b[0]) <= t && Math.abs(a[1] - b[1]) <= t && Math.abs(a[2] - b[2]) <= t;
const average = (frame) => {
  let r = 0, g = 0, b = 0;
  const n = frame.length / 3;
  for (let i = 0; i < frame.length; i += 3) { r += frame[i]; g += frame[i + 1]; b += frame[i + 2]; }
  return [r / n, g / n, b / n];
};
/** Black text on a white page, as Wikipedia's heading is: the app's own dark background isn't. */
const headingDrawn = (frame) => {
  let dark = 0;
  let white = 0;
  const n = frame.length / 3;
  for (let i = 0; i < frame.length; i += 3) {
    if (frame[i] < 70 && frame[i + 1] < 70 && frame[i + 2] < 70) dark++;
    if (frame[i] > 225 && frame[i + 1] > 225 && frame[i + 2] > 225) white++;
  }
  return dark / n > 0.02 && white / n > 0.4;
};

/** Records around one action; each frame's time and what's in the two places looked at. */
async function recordAction(tag, action, seconds = 2.6, region = REGION) {
  const out = path.join(folder, `${tag}.mp4`);
  const rec = spawn(path.join(binDir, "winrec"), [windowId, seconds, out, 120, 1].map(String), { stdio: ["ignore", "pipe", "inherit"] });
  await new Promise((resolve, reject) => {
    readline.createInterface({ input: rec.stdout }).on("line", (line) => { if (line.startsWith("recording")) resolve(); });
    rec.on("exit", (code) => { if (code) reject(new Error(`winrec exited ${code}`)); });
  });
  await sleep(350);
  const sentAt = Number(process.hrtime.bigint()) / 1e9;
  await action();
  await new Promise((resolve) => rec.on("exit", resolve));
  const times = fs.readFileSync(`${out}.times`, "utf8").trim().split("\n").map((line) => Number(line.split(" ")[1]));
  const places = await framesOf(out, region);
  if (places.length !== times.length) throw new Error(`${tag}: ${places.length} frames but ${times.length} times`);
  if (Math.abs(times[0] - sentAt) > 5) throw new Error(`${tag}: frames (${times[0]}) and this script (${sentAt}) aren't on one clock`);
  return { sentAt, frames: times.map((time, index) => ({ time, place: places[index] })), out };
}
const elapsed = ({ sentAt, frames }, reached) => {
  for (const frame of frames) {
    if (frame.time >= sentAt && reached(frame.place)) return { ms: Math.round((frame.time - sentAt) * 1000) };
  }
  return { error: "never reached" };
};
const remember = (key, how, runs) => {
  const ms = runs.filter((run) => run.ms !== undefined).map((run) => run.ms).sort((a, b) => a - b);
  const entry = { how, n: ms.length, failed: runs.length - ms.length, min: ms[0], median: ms[Math.floor(ms.length / 2)], max: ms.at(-1), all: runs.map((run) => run.ms ?? run.error) };
  results.timings[key] = entry;
  save();
  log(key, JSON.stringify({ n: entry.n, failed: entry.failed, min: entry.min, median: entry.median, max: entry.max }));
};
const colorOf = async (tag) => {
  await sleep(400);
  const { image } = await capture(windowId, `calibrate-${tag}`, folder);
  return image.at(REGION.x * 2 + 16, REGION.y * 2 + 16);
};
const tabsApi = `const s=await import("/src/state/sessionTabs.ts");`;
const closeTabById = (id) => host(name, `(async()=>{${tabsApi}s.closeTab(${JSON.stringify(session)},${JSON.stringify(id)});return 1})()`);
const activeTab = () => host(name, `(async()=>{${tabsApi}return s.sessionTabs(${JSON.stringify(session)}).active})()`);
const openNow = (url) => `(async()=>{${tabsApi}return s.openBrowserTab(${JSON.stringify(session)},${JSON.stringify(url)})})()`;
const send = (event) => control("hostSendInput", { name, event });
const typeAddress = async (url) => {
  const r = await host(name, `(()=>{const e=document.querySelector('[data-pane="browser"][data-active] input[aria-label="Address"]');const r=e.getBoundingClientRect();return {x:r.x+100,y:r.y+10}})()`);
  const at = { x: Math.round(r.x), y: Math.round(r.y) };
  await send({ type: "mouseDown", ...at, button: "left", clickCount: 1 });
  await send({ type: "mouseUp", ...at, button: "left", clickCount: 1 });
  await sleep(350);
  await host(name, `document.activeElement?.select?.();1`);
  await control("hostInsertText", { name, text: url });
  await sleep(100);
};
const pressEnter = async () => {
  await send({ type: "keyDown", keyCode: "Enter" });
  await send({ type: "char", keyCode: "\r" });
  await send({ type: "keyUp", keyCode: "Enter" });
};

// What the pipe itself adds: a round trip that does nothing, to the page and to the main process.
{
  const trip = async (run) => {
    const all = [];
    for (let n = 0; n < 30; n++) { const start = process.hrtime.bigint(); await run(); all.push(Number(process.hrtime.bigint() - start) / 1e6); }
    all.sort((a, b) => a - b);
    return { median: Number(all[15].toFixed(1)), max: Number(all[29].toFixed(1)) };
  };
  results.pipeRoundTripMs = { page: await trip(() => host(name, "1")), mainProcess: await trip(() => control("hostCapturing", { name })) };
  save();
  log("pipe round trip", JSON.stringify(results.pipeRoundTripMs));
}

// Home tab: a still page in a known colour.
const home = await openTab(name, session, `${FIXTURES}/still/204080/home`);
const homeColor = await colorOf("home");
log("home colour", homeColor);

// ⌘T to the New Tab layout on screen.
if (want("newtab")) {
  const runs = [];
  for (let n = 0; n < 8; n++) {
    const recorded = await recordAction(`new-tab-ui-${n}`, async () => { await send({ type: "keyDown", keyCode: "t", modifiers: ["meta"] }); await send({ type: "keyUp", keyCode: "t", modifiers: ["meta"] }); }, 1.6);
    runs.push(elapsed(recorded, (place) => !near(average(place), homeColor, 40)));
    await closeTabById(await activeTab());
    await selectTab(name, session, home.id);
    await sleep(300);
  }
  remember("New Tab: cmd-T to its layout drawn", "From sending the key to the first frame the page area is no longer the old page", runs);
}

// A new tab with an address: first paint of the page.
for (const [key, url, reached, count, region] of !want("newtab") ? [] : [
  ["New tab with a local page: to first paint", `${FIXTURES}/still/802040/new`, null, 8, REGION],
  ["New tab with Wikipedia: to its heading drawn (warm cache)", "https://en.wikipedia.org/wiki/Web_browser", headingDrawn, 6, HEADING],
]) {
  const target = reached ? null : await (async () => {
    const tab = await openTab(name, session, url);
    const color = await colorOf("new");
    await closeTabById(tab.id);
    await selectTab(name, session, home.id);
    await sleep(300);
    return color;
  })();
  if (reached) {
    const warm = await openTab(name, session, url);
    await sleep(2500);
    await closeTabById(warm.id);
    await selectTab(name, session, home.id);
    await sleep(300);
  }
  const runs = [];
  const fcp = [];
  for (let n = 0; n < count; n++) {
    let opened = null;
    const recorded = await recordAction(`new-tab-page-${key.slice(13, 18)}-${n}`, async () => { opened = await host(name, openNow(url)); }, reached ? 4 : 2.2, region);
    runs.push(elapsed(recorded, reached ?? ((place) => near(average(place), target, 40))));
    const page = (await layerState(name)).pages.find((each) => each.id === opened);
    if (page?.wc) fcp.push(await guestExec(page.wc, `Math.round(performance.getEntriesByName("first-contentful-paint")[0]?.startTime ?? -1)`));
    await closeTabById(opened);
    await selectTab(name, session, home.id);
    await sleep(400);
  }
  remember(key, "From sending openBrowserTab to the first frame showing the page", runs);
  results.timings[key].guestFirstContentfulPaintMs = fcp;
}

// Tab switch. Each page paints a new colour every time it's shown, so the time to that colour can't be the
// old surface coming back: it's a live renderer told it's visible, painting, and that reaching the screen.
if (want("switch")) {
  const one = await openTab(name, session, `${FIXTURES}/fresh/aa2222/22aa22/one`);
  const two = await openTab(name, session, `${FIXTURES}/fresh/2222aa/aaaa22/two`);
  const learn = async (tab) => {
    const colors = [];
    for (let n = 0; n < 2; n++) {
      await selectTab(name, session, home.id);
      await sleep(250);
      await selectTab(name, session, tab.id);
      await sleep(400);
      const shows = await guestExec(tab.wc, "window.__shows");
      colors[shows % 2] = await colorOf(`fresh-${shows % 2}`);
    }
    return colors;
  };
  const colorsOf = new Map([[one, await learn(one)], [two, await learn(two)]]);
  const pixelsRuns = [];
  const freshRuns = [];
  const stale = [];
  const strays = [];
  await selectTab(name, session, one.id);
  await sleep(300);
  for (let n = 0; n < 20; n++) {
    const [to, from] = n % 2 ? [one, two] : [two, one];
    const fromColors = colorsOf.get(from);
    const fromColor = fromColors[(await guestExec(from.wc, "window.__shows")) % 2];
    const shows = await guestExec(to.wc, "window.__shows");
    const target = colorsOf.get(to)[(shows + 1) % 2];
    const oldSurface = colorsOf.get(to)[shows % 2];
    await sleep(250);
    const recorded = await recordAction(`switch-${n}`, async () => host(name, `(async()=>{${tabsApi}s.selectTab(${JSON.stringify(session)},${JSON.stringify(to.id)});return 1})()`), 1.2);
    pixelsRuns.push(elapsed(recorded, (place) => !near(average(place), fromColor, 40)));
    freshRuns.push(elapsed(recorded, (place) => near(average(place), target, 40)));
    const fresh = recorded.frames.findIndex((frame) => frame.time >= recorded.sentAt && near(average(frame.place), target, 40));
    const after = recorded.frames.filter((frame, index) => frame.time >= recorded.sentAt && (fresh < 0 || index < fresh));
    stale.push(after.filter((frame) => near(average(frame.place), oldSurface, 40)).length);
    strays.push(after.filter((frame) => { const c = average(frame.place); return !near(c, fromColor, 40) && !near(c, oldSurface, 40); }).length);
  }
  remember("Tab switch: to the new tab's last picture on screen", "From sending selectTab to the first frame the area is no longer the old tab's. May be stale: the surface kept from its last showing", pixelsRuns);
  remember("Tab switch: to fresh paint by the new tab's renderer", "From sending selectTab to the first frame with a colour the page paints only after being told it's visible this time", freshRuns);
  results.timings["Tab switch: to fresh paint by the new tab's renderer"].framesOfOldSurfaceBeforeFresh = stale;
  results.timings["Tab switch: to fresh paint by the new tab's renderer"].framesOfAnythingElse = strays;
  save();
  await selectTab(name, session, home.id);
  await closeTabById(one.id);
  await closeTabById(two.id);
  await selectTab(name, session, home.id);
}

// Address bar navigation.
if (want("address")) {
  const a = await openTab(name, session, `${FIXTURES}/still/206040/a`);
  await sleep(400);
  const aColor = await colorOf("a");
  const runs = [];
  const bColorTab = await openTab(name, session, `${FIXTURES}/still/602060/b`);
  const bColor = await colorOf("b");
  await closeTabById(bColorTab.id);
  await selectTab(name, session, a.id);
  await sleep(300);
  for (let n = 0; n < 8; n++) {
    const [url, target] = n % 2 ? [`${FIXTURES}/still/206040/a`, aColor] : [`${FIXTURES}/still/602060/b`, bColor];
    await typeAddress(url);
    const recorded = await recordAction(`address-${n}`, pressEnter, 1.8);
    runs.push(elapsed(recorded, (place) => near(average(place), target, 40)));
    await sleep(200);
  }
  remember("Address bar, Return to first paint: local page", "From sending Return to the first frame in the new page's colour", runs);
  const wikiRuns = [];
  await typeAddress("https://en.wikipedia.org/wiki/Web_browser");
  await pressEnter();
  await sleep(2500);
  for (let n = 0; n < 5; n++) {
    await typeAddress(`${FIXTURES}/still/206040/a`);
    await pressEnter();
    await sleep(600);
    await typeAddress("https://en.wikipedia.org/wiki/Web_browser");
    const recorded = await recordAction(`address-wiki-${n}`, pressEnter, 3.5, HEADING);
    wikiRuns.push(elapsed(recorded, headingDrawn));
    await sleep(300);
  }
  remember("Address bar, Return to first paint: Wikipedia (warm cache)", "From sending Return to the first frame with its heading", wikiRuns);
  await closeTabById(a.id);
  await selectTab(name, session, home.id);
}

// Input to paint in a page.
if (want("input")) {
  const tab = await openTab(name, session, `${FIXTURES}/input/2040a0/a02040`);
  await sleep(400);
  const clickRuns = [];
  const keyRuns = [];
  const colors = [await colorOf("input-a")];
  const flip = (type, extra) => control("guestSendInput", { id: tab.wc, event: { type, ...extra } });
  await flip("mouseDown", { x: 300, y: 300, button: "left", clickCount: 1 });
  await flip("mouseUp", { x: 300, y: 300, button: "left", clickCount: 1 });
  colors.push(await colorOf("input-b"));
  let at = 1;
  const runOne = async (kind, tag) => {
    const target = colors[(at + 1) % 2];
    const recorded = await recordAction(tag, async () => {
      if (kind === "click") {
        await flip("mouseDown", { x: 300, y: 300, button: "left", clickCount: 1 });
        await flip("mouseUp", { x: 300, y: 300, button: "left", clickCount: 1 });
      } else {
        await flip("keyDown", { keyCode: "a" });
        await flip("keyUp", { keyCode: "a" });
      }
    }, 1);
    at = (at + 1) % 2;
    return elapsed(recorded, (place) => near(average(place), target, 40));
  };
  for (let n = 0; n < 12; n++) clickRuns.push(await runOne("click", `input-click-${n}`));
  for (let n = 0; n < 12; n++) keyRuns.push(await runOne("key", `input-key-${n}`));
  remember("Input to paint in a page: scripted mouse press", "From sending sendInputEvent to the first frame in the page's new colour. No real pointer", clickRuns);
  remember("Input to paint in a page: scripted key", "same way, a key instead of a press", keyRuns);
  const timing = await guestExec(tab.wc, "__timing");
  results.timings["Event Timing, as the page itself reports it"] = { how: "PerformanceEventTiming durations of at least 16 ms (the API's floor), in 8 ms steps; events faster than 16 ms aren't listed", entries: timing.length, durations: timing.map(([event, ms]) => `${event} ${ms}`).slice(0, 50) };
  save();
  await closeTabById(tab.id);
  await selectTab(name, session, home.id);
}
await closeTabById(home.id);
await sleep(400);
await host(name, "localStorage.clear();1");
await control("closeWindow", { name });

// Idle CPU and memory, on real sites, with nothing recording.
const SITES = [
  "https://en.wikipedia.org/wiki/Web_browser", "https://news.ycombinator.com/", "https://github.com/electron/electron",
  "https://developer.mozilla.org/en-US/", "https://www.python.org/", "https://www.rust-lang.org/",
  "https://stackoverflow.com/questions", "https://www.bbc.com/news", "https://www.apple.com/", "https://example.com/",
];
const idleName = "idle";
const idleSession = `session-${idleName}`;
const cpuSeconds = (text) => {
  const [minutes, seconds] = text.trim().split(":").map(Number);
  return minutes * 60 + seconds;
};
const appPids = async () => (await control("metrics")).map((metric) => metric.pid);
const cpuOf = (pids) => Object.fromEntries(pids.map((pid) => {
  try { return [pid, cpuSeconds(execFileSync("/bin/ps", ["-o", "time=", "-p", String(pid)]).toString())]; } catch { return [pid, null]; }
}));
const footprintOf = (pid) => {
  try {
    const match = /phys_footprint: ([\d.]+) (KB|MB|GB)/.exec(execFileSync("/usr/bin/footprint", [String(pid)], { stdio: ["ignore", "pipe", "ignore"] }).toString());
    return match ? Number(match[1]) * { KB: 1 / 1024, MB: 1, GB: 1024 }[match[2]] : null;
  } catch { return null; }
};
async function measureIdle(tabs, state, seconds = 60) {
  await sleep(20000);
  const before = cpuOf(await appPids());
  const started = Date.now();
  await sleep(seconds * 1000);
  const wall = (Date.now() - started) / 1000;
  const metrics = await control("metrics");
  const after = cpuOf(metrics.map((metric) => metric.pid));
  const usedBy = (metric) => Math.max(0, (after[metric.pid] ?? 0) - (before[metric.pid] ?? after[metric.pid] ?? 0));
  const used = metrics.reduce((sum, metric) => sum + usedBy(metric), 0);
  const row = {
    tabs,
    state,
    cpuPercentOfOneCore: Number(((used / wall) * 100).toFixed(1)),
    seconds: Math.round(wall),
    footprintMb: Math.round(metrics.reduce((sum, metric) => sum + (footprintOf(metric.pid) ?? 0), 0)),
    residentMb: Math.round(metrics.reduce((sum, metric) => sum + (metric.kb ?? 0), 0) / 1024),
    renderers: metrics.filter((metric) => metric.type === "Tab").length,
    cpuByType: Object.fromEntries(["Browser", "GPU", "Utility", "Tab"].map((type) => [type, Number(((metrics.filter((metric) => metric.type === type).reduce((sum, metric) => sum + usedBy(metric), 0) / wall) * 100).toFixed(1))])),
    busiestRenderers: metrics.filter((metric) => metric.type === "Tab").map((metric) => [metric.pid, Number(((usedBy(metric) / wall) * 100).toFixed(1))]).sort((x, y) => y[1] - x[1]).slice(0, 3),
    byType: Object.fromEntries(["Browser", "GPU", "Utility", "Tab"].map((type) => [type, Math.round(metrics.filter((metric) => metric.type === type).reduce((sum, metric) => sum + (footprintOf(metric.pid) ?? 0), 0))])),
  };
  results.idle.push(row);
  save();
  log("idle", JSON.stringify(row));
}
if (want("idle")) {
  results.idle = [];
  await openShell(idleName);
  await host(idleName, "localStorage.clear();1");
  await control("closeWindow", { name: idleName });
  await openShell(idleName);
  await selectSession(idleName, idleSession);
  await sleep(800);
  await measureIdle(0, "shown");
  let open = 0;
  for (const target of [1, 3, 10]) {
    while (open < target) {
      await openTab(idleName, idleSession, SITES[open]);
      await sleep(3500);
      open++;
    }
    await measureIdle(target, "shown");
  }
  const { display } = await control("info");
  const cover = startCover(display.width - 40, 0, 40, display.height - 33);
  await control("setLevel", { name: idleName, level: "normal" });
  await measureIdle(10, "behind another window");
  cover.kill();
  await control("setLevel", { name: idleName, level: "floating" });
  await host(idleName, "localStorage.clear();1");
  await control("closeWindow", { name: idleName });
}
// Whose CPU it is, with one tab and nothing in it that could be busy, then a real site.
if (want("cpu")) {
  await openShell(idleName);
  await host(idleName, "localStorage.clear();1");
  await control("closeWindow", { name: idleName });
  await openShell(idleName);
  await selectSession(idleName, idleSession);
  await sleep(800);
  results.cpu = [];
  const row = async (label) => { await measureIdle(0, label, 45); results.cpu.push({ label, ...results.idle.pop() }); save(); };
  await row("session open, no tab");
  await openTab(idleName, idleSession, `${FIXTURES}/still/204080/still`);
  await sleep(500);
  await row("one still local page in front");
  // What's left if the app's per-frame loops stop: rAF becomes a no-op in this window only.
  await host(idleName, "window.__raf=window.requestAnimationFrame;window.requestAnimationFrame=()=>0;1");
  await row("same, with the host page's requestAnimationFrame stopped");
  await host(idleName, "window.requestAnimationFrame=window.__raf;1");
  await openTab(idleName, idleSession, "https://en.wikipedia.org/wiki/Web_browser");
  await sleep(500);
  await row("Wikipedia in front");
  await selectSession(idleName, `${idleSession}-other`);
  await sleep(500);
  await row("another session, no page showing");
  await host(idleName, "localStorage.clear();1");
  await control("closeWindow", { name: idleName });
}
log("done", path.join(folder, "results.json"));
process.exit(0);
