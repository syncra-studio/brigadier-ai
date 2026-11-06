#!/usr/bin/env node
// Every check that needs the real pointer, the real cursor or real keyboard focus, in one go. These take
// over the Mac for about three minutes, so they run only when the user says go:
//
//   go.mjs --plan        lists what it does and proves, touches nothing
//   go.mjs --preflight   everything up to the first real input, with the window on its strip, unactivated:
//                        no focus taken, no pointer moved, works without the permission; then restores
//   go.mjs --go          runs it
//
// The first mouse or key event that isn't its own ends the run: the pointer goes back where it was, the
// app that was in front comes back, and it reports what it had got through. Real input comes from its own
// small app, "Brigadier Check Input.app", which needs Accessibility. --go asks that app first, and without
// the permission touches nothing. It never prompts.
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import readline from "node:readline";

import { call } from "./bc.mjs";
import { appBundle, checkDir, checkInputApp, eventsPath, evidenceDir } from "./paths.mjs";
import { framesOf, osWindows, record, windowIdOf, capture } from "./instruments.mjs";
import { FIXTURES, control, guestExec, host, layerState, openShell, openTab, selectSession, setSidebarWidth, sleep, spaces } from "./lib.mjs";

const CHECKS = [
  ["focus", "Clicking the window makes the app active and the window key; the page reports hasFocus.", "macOS gives focus to a real click, in an accessory app rather than the background-only dev mode. A regular Dock app stays unproved."],
  ["click-in-page", "A real click in the page's field reaches the guest as a trusted event and focuses it.", "Hit-testing through the hole in #root and the tab layer with the real pointer."],
  ["typing", "Real keys type into the page, and only the page.", "Keyboard focus lands in the guest."],
  ["address", "Real click in the address bar, real typing, Return: the page navigates.", "Focus moves from guest to app and back."],
  ["hover-cursor", "Over the link macOS shows the hand, over the field the I-beam, over the page the arrow.", "The cursor really changes: the one thing a guest's cursor-changed event can't prove."],
  ["scroll", "A real wheel scrolls the page and the recording shows steady motion.", "Wheel routing and no dropped frames while scrolling."],
  ["menu", "A real click opens the HTML menu over the page, a click beside it closes it without reaching the page, and a real click on Find in page opens the find bar.", "Clicks on HTML over a live page."],
  ["divider", "A real drag of the sidebar divider: the page follows every frame, gets no pointer events, shows the resize cursor.", "Divider drag, page locked, by hand."],
  ["window-drag", "A real drag of the empty strip right of the tabs, recorded: the window moves, the page stays at the same place in it in every frame, gets no pointer events.", "Window drag, page locked."],
  ["shortcuts", "Real cmd-T and cmd-shift-B open tabs.", "The app's shortcuts with real keys and focus."],
  ["minimise", "The real yellow button minimises; restoring (by script: a Dock tile can't be found without another grant) shows the same tab, live.", "Minimise from the real control."],
  ["another-space", "A real Ctrl-arrow moves to the neighbouring Space for 40 s while the check reads host, guest and capturePage and switches tab, then the opposite one returns; the page is live and the same document.", "The Space case the gate can't enter alone, and the last piece of the spike's blank state."],
];

const argument = process.argv[2];
if (argument !== "--go" && argument !== "--preflight") {
  console.log("Would run, in order (real input, real focus, about 3 minutes):\n");
  for (const [id, what, proves] of CHECKS) console.log(`- ${id}: ${what}\n    proves: ${proves}`);
  console.log("\nStops on any input that isn't its own, puts the pointer and front app back. Run with --go.");
  process.exit(0);
}

const plist = path.join(appBundle, "Contents/Info.plist");
const folder = path.join(evidenceDir, `${argument.slice(2)}-${new Date().toISOString().replace(/[:.]/g, "-")}`);
fs.mkdirSync(folder, { recursive: true });
const results = [];
const check = (id, passed, detail = {}) => {
  results.push({ id, passed, proof: argument === "--go" ? "real focused" : "preflight, no real input", ...detail });
  console.log(`${passed ? "ok  " : "FAIL"} ${id} ${JSON.stringify(detail).slice(0, 200)}`);
  fs.writeFileSync(path.join(folder, "results.json"), JSON.stringify(results, null, 1));
};

/** Starts the input app through LaunchServices, so it's its own responsible process. Its first line is hello. */
function startInputApp(extra = []) {
  const socketPath = path.join(checkDir, "check-input.sock");
  fs.rmSync(socketPath, { force: true });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${checkInputApp} didn't connect`)), 15000);
    const server = net.createServer((socket) => {
      const lines = readline.createInterface({ input: socket });
      let hello = null;
      const channel = { socket, onLine: null, onEnd: null };
      lines.on("line", (line) => {
        const value = JSON.parse(line);
        if (!hello) {
          hello = value;
          clearTimeout(timer);
          resolve({ hello, channel, server });
        } else channel.onLine?.(value);
      });
      socket.on("close", () => channel.onEnd?.());
      socket.on("error", () => {});
    });
    server.listen(socketPath, () => {
      execFileSync("/usr/bin/open", ["-g", "-n", checkInputApp, "--args", "--socket", socketPath, ...extra]);
    });
  });
}
const askInputApp = async () => {
  const { hello, channel, server } = await startInputApp(["--check"]);
  channel.socket.destroy();
  server.close();
  return hello;
};

const before = await askInputApp();
console.log(`input app: trusted=${before.trusted} pointer=${before.pointer} front=${before.front} cursor=${before.cursor}`);
if (argument === "--go" && !before.trusted) {
  console.log(`Nothing was touched. Turn Accessibility on for ${checkInputApp}, then run --go again.`);
  process.exit(4);
}

// The app can't be front while background-only. As a regular app launched without activation, macOS keeps
// its window off screen (preflight found this), so it's an accessory app for this run: its window shows
// unactivated, and a real click makes it active. Put back afterwards.
const setForChecks = (on) => {
  const edit = (args) => execFileSync("plutil", args);
  if (on) {
    edit(["-remove", "LSBackgroundOnly", plist]);
    edit(["-insert", "LSUIElement", "-bool", "true", plist]);
  } else {
    edit(["-remove", "LSUIElement", plist]);
    edit(["-insert", "LSBackgroundOnly", "-bool", "true", plist]);
  }
  execFileSync("codesign", ["--force", "--deep", "--sign", "-", appBundle], { stdio: "ignore" });
};
await call({ op: "down" }).catch(() => {});
await sleep(3000);
setForChecks(true);
const up = spawn(process.execPath, [new URL("./bc.mjs", import.meta.url).pathname, "up"], { stdio: "ignore", env: { ...process.env, BRIGADIER_FOCUSED: "1" } });
await new Promise((resolve) => up.on("exit", resolve));

let waiting = null;
let stopped = null;
let inputChannel = null;
if (argument === "--go") {
  const { channel } = await startInputApp();
  inputChannel = channel;
  channel.onLine = (value) => {
    if (value.human !== undefined || value.error) {
      stopped = value;
      return;
    }
    waiting?.(value);
    waiting = null;
  };
  channel.onEnd = () => {
    stopped ??= { exit: "the input app went away" };
    waiting?.({ stopped: true });
  };
}
const real = (command) => {
  if (stopped) throw new Error(`stopped: ${JSON.stringify(stopped)}`);
  return new Promise((resolve) => {
    waiting = resolve;
    inputChannel.socket.write(`${JSON.stringify(command)}\n`);
  });
};

let awayFrom = null;
async function finish(code) {
  // Left on another Space by a failure: go back, unless a person is at the keyboard.
  if (awayFrom && !stopped) {
    try { await real({ op: "key", code: awayFrom.back, flags: ["ctrl"] }); } catch { /* reported below */ }
  }
  if (inputChannel) {
    try { inputChannel.socket.write(`${JSON.stringify({ op: "restore" })}\n`); } catch { /* already gone */ }
    await sleep(500);
  }
  await call({ op: "down" }).catch(() => {});
  await sleep(3000);
  setForChecks(false);
  // The kept-alive instance comes back as it was: background-only, unfocusable.
  const { BRIGADIER_FOCUSED: _focused, ...env } = process.env;
  const again = spawn(process.execPath, [new URL("./bc.mjs", import.meta.url).pathname, "up"], { stdio: "ignore", env });
  await new Promise((resolve) => again.on("exit", resolve));
  console.log(stopped?.human !== undefined ? "STOPPED: human input. Pointer and front app restored." : stopped?.error ?? "done", path.join(folder, "results.json"));
  process.exit(code);
}

// Every pointer step goes through these: a target outside the window, or not on the element it's meant to
// be, is an error before any input is posted. The real run's point must also be on the screen.
function pointTools(name, tab, { onScreen }) {
  const windowAt = async () => (await control("windowInfo", { name })).bounds;
  const inside = async (x, y, what) => {
    const bounds = await windowAt();
    const screen = (await control("info")).display;
    const sx = bounds.x + x;
    const sy = bounds.y + y;
    if (x < 0 || y < 0 || x >= bounds.width || y >= bounds.height) throw new Error(`${what}: (${Math.round(x)}, ${Math.round(y)}) is outside the ${bounds.width}x${bounds.height} window`);
    if (onScreen && (sx < 0 || sy < 0 || sx >= screen.width || sy >= screen.height)) throw new Error(`${what}: (${Math.round(sx)}, ${Math.round(sy)}) is off the screen`);
    return { x: sx, y: sy };
  };
  const pageRect = async () => (await layerState(name)).pages.find((page) => page.id === tab.id).rect;
  /** A point in the page, given by an element's centre or by page coordinates. The element under it must be `expect`. */
  const inPage = async (where, expect) => {
    const local = typeof where === "string"
      ? await guestExec(tab.wc, `(()=>{const r=document.querySelector(${JSON.stringify(where)}).getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`)
      : where;
    const page = await pageRect();
    const point = await inside(page.x + local.x, page.y + local.y, `page point for ${expect}`);
    if (local.x >= page.width || local.y >= page.height) throw new Error(`page point for ${expect}: outside the ${page.width}x${page.height} page`);
    const under = await guestExec(tab.wc, `(()=>{const e=document.elementFromPoint(${local.x},${local.y});return {hit:Boolean(e&&e.closest(${JSON.stringify(expect)})),tag:e?(e.id||e.tagName):null}})()`);
    if (!under.hit) throw new Error(`page point for ${expect}: elementFromPoint finds ${under.tag}`);
    return point;
  };
  /** A point in the app's own page, in window coordinates. The element under it must be `expect`. */
  const inApp = async (x, y, expect) => {
    const point = await inside(x, y, `app point for ${expect}`);
    const under = await host(name, `(()=>{const e=document.elementFromPoint(${x},${y});return {hit:Boolean(e&&e.closest(${JSON.stringify(expect)})),tag:e?(e.getAttribute("data-slot")||e.tagName):null}})()`);
    if (!under.hit) throw new Error(`app point for ${expect}: elementFromPoint finds ${under.tag}`);
    return point;
  };
  const centreOf = (selector) => host(name, `(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)return null;const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
  /** The middle of what's truly empty and draggable: the gap right of the tabs. */
  const dragStrip = async () => {
    const strip = await host(name, `(()=>{const all=[...document.querySelectorAll("[data-tauri-drag-region]")].filter((e)=>e.childElementCount===0).map((e)=>({e,r:e.getBoundingClientRect()})).filter(({r})=>r.width>40&&r.height>10&&r.y<60).sort((a,b)=>b.r.width-a.r.width);if(!all.length)return null;const {r}=all[0];return {x:r.x+r.width/2,y:r.y+r.height/2,width:r.width}})()`);
    if (!strip) throw new Error("no empty drag strip found");
    const under = await host(name, `document.elementFromPoint(${strip.x},${strip.y}).hasAttribute("data-tauri-drag-region")`);
    if (!under) throw new Error("the drag strip's centre is covered");
    return { strip, point: await inside(strip.x, strip.y, "drag strip") };
  };
  const toolbarButton = () => host(name, `(()=>{const all=document.querySelectorAll('[data-pane="browser"][data-active] [data-slot="browser-toolbar"] button');const r=all[all.length-1].getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
  const pageMiddle = async () => {
    const size = await guestExec(tab.wc, "({w:innerWidth,h:innerHeight})");
    return { x: Math.round(size.w / 2), y: Math.round(size.h / 2) };
  };
  const spaceStep = () => {
    const { active, managed } = spaces("list");
    const at = managed.indexOf(active);
    if (at < 0) return { active, managed, error: "the active Space isn't in the list" };
    if (at < managed.length - 1) return { active, managed, go: 124, back: 123, direction: "right" };
    if (at > 0) return { active, managed, go: 123, back: 124, direction: "left" };
    return { active, managed, error: "there's only one Space" };
  };
  return { pageRect, inPage, inApp, centreOf, dragStrip, toolbarButton, pageMiddle, spaceStep };
}

// Up to the first real input, on the strip: does the window really show when the app is a regular app?
if (argument === "--preflight") {
  try {
    const name = "go";
    const startedAt = new Date().toISOString();
    const eventsSince = () => fs.readFileSync(eventsPath, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)).filter((row) => row.at >= startedAt);
    const watchSince = () => fs.readFileSync(path.join(checkDir, "watch.jsonl"), "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)).filter((row) => row.at >= startedAt);
    const { pid } = await control("info");
    const type = execFileSync("/bin/sh", ["-c", `lsappinfo list | grep -A6 'pid = ${pid} ' | grep -o 'type="[A-Za-z]*"' | head -1`]).toString().trim();
    check("the app is an accessory app for this, no longer background-only", type === 'type="UIElement"', { type });
    const { windowId } = await openShell(name, { level: "floating", allSpaces: true });
    await control("ignoreMouse", { name, on: true });
    await sleep(500);
    const info = await control("windowInfo", { name });
    const shows = eventsSince().filter((row) => row.kind === "window-show" && row.window === name);
    const os = (await osWindows(pid)).find((row) => row.id === windowId);
    check("the window's show event fires", shows.length >= 1, { shows: shows.map((row) => row.at) });
    check("the OS lists it on screen, on the strip", os?.onscreen === true && os.bounds.X === info.bounds.x && info.visible, { os, bounds: info.bounds });
    check("it isn't focused and the app isn't front", info.focused === false, { focused: info.focused });
    // If it isn't on screen: is it late, or does something bring it? None of these activates the app.
    const sample = async (label) => {
      const row = { label, ...(await control("windowDiag", { name })), onscreen: (await osWindows(pid)).find((each) => each.id === windowId)?.onscreen ?? null, host: await host(name, "document.visibilityState"), showEvents: eventsSince().filter((each) => each.kind === "window-show" && each.window === name).length };
      console.log(JSON.stringify(row));
      return row;
    };
    const diagnosis = [await sample("right after showInactive")];
    await sleep(3000);
    diagnosis.push(await sample("3 s later"));
    await control("showInactiveAgain", { name });
    await sleep(800);
    diagnosis.push(await sample("showInactive again"));
    await control("moveTop", { name });
    await sleep(800);
    diagnosis.push(await sample("moveTop"));
    await control("showApp");
    await sleep(800);
    diagnosis.push(await sample("app.show"));
    await control("setBounds", { name, bounds: info.bounds });
    results.push({ id: "window diagnosis", diagnosis });
    const session = `session-${name}`;
    await selectSession(name, session);
    const tab = await openTab(name, session, `${FIXTURES}/page/336699/go`);
    await sleep(1000);
    const first = await capture(windowId, "window-on-strip", folder);
    const page = first.image.at(1400, 800);
    check("the OS captures the window, with its page", page[2] > page[0] + 30, { page, image: first.file });
    await guestExec(tab.wc, `__paintNonce("ff00ff");1`);
    await sleep(400);
    const fresh = await capture(windowId, "window-fresh-paint", folder);
    const band = fresh.image.at(1400, 1540);
    check("the page in it is live: a colour chosen now reaches the OS's pixels", band[0] > 180 && band[1] < 100, { band, image: fresh.file });
    // Every target the real run will use, resolved and checked, with no input to the OS.
    const tools = pointTools(name, tab, { onScreen: false });
    const resolves = async (id, resolve) => {
      try { check(`target: ${id}`, true, { point: await resolve() }); } catch (error) { check(`target: ${id}`, false, { error: String(error.message) }); }
    };
    const middle = await tools.pageMiddle();
    await resolves("link", () => tools.inPage("#link", "#link"));
    await resolves("field", () => tools.inPage("#field", "#field"));
    await resolves("page", () => tools.inPage(middle, "#tall"));
    await resolves("address bar", async () => { const c = await tools.centreOf('[data-pane="browser"][data-active] input[aria-label="Address"]'); return tools.inApp(c.x, c.y, 'input[aria-label="Address"]'); });
    await resolves("divider", async () => { const c = await tools.centreOf('[data-slot="sidebar-resize-handle"]'); return tools.inApp(c.x, 400, '[data-slot="sidebar-resize-handle"]'); });
    await resolves("drag strip", async () => (await tools.dragStrip()).point);
    const button = await tools.toolbarButton();
    await resolves("options button", () => tools.inApp(button.x, button.y, "button"));
    const appInput = (event) => control("hostSendInput", { name, event });
    const at = { x: Math.round(button.x), y: Math.round(button.y) };
    await appInput({ type: "mouseDown", ...at, button: "left", clickCount: 1 });
    await appInput({ type: "mouseUp", ...at, button: "left", clickCount: 1 });
    await sleep(400);
    const item = await host(name, `(()=>{const e=[...document.querySelectorAll("[role=menuitem]")].find((x)=>x.textContent.trim()==="Find in page");if(!e)return null;const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2,disabled:e.hasAttribute("data-disabled")}})()`);
    check("target: Find in page, enabled, in the open menu", item !== null && !item.disabled, { item });
    if (item) await resolves("Find in page's point", () => tools.inApp(item.x, item.y, "[role=menuitem]"));
    await appInput({ type: "keyDown", keyCode: "Escape" });
    await appInput({ type: "keyUp", keyCode: "Escape" });
    await sleep(300);
    const plan = tools.spaceStep();
    check("target: a neighbouring Space to go to", !plan.error, plan);

    // Bad targets must fail loudly, as the old one didn't: its centre of #tall sent the pointer off the window.
    const refuses = async (id, resolve) => {
      try { const point = await resolve(); check(`refuses: ${id}`, false, { point }); } catch (error) { check(`refuses: ${id}`, true, { error: String(error.message) }); }
    };
    await refuses("the old hover target, #tall's centre", () => tools.inPage("#tall", "#tall"));
    await refuses("a point on the wrong element", () => tools.inPage("#link", "#field"));
    await refuses("the old drag point, on a tab", () => tools.inApp(640, 12, "[data-tauri-drag-region]:not(:has(*))"));

    // The scroll oracle, by scripted wheel: it says yes when the wheel reaches the page, no when it doesn't.
    await guestExec(tab.wc, "scrollTo(0,0);__fixture.wheel=0;1");
    await control("guestSendInput", { id: tab.wc, event: { type: "mouseWheel", x: middle.x, y: middle.y, deltaX: 0, deltaY: -500, canScroll: true } });
    await sleep(400);
    const hit = await guestExec(tab.wc, "({scrollY,wheel:__fixture.wheel})");
    check("scroll oracle: a wheel on the page moves it", hit.scrollY > 100 && hit.wheel > 0, hit);
    await guestExec(tab.wc, "scrollTo(0,0);__fixture.wheel=0;1");
    await sleep(100);
    await appInput({ type: "mouseWheel", x: 100, y: 700, deltaX: 0, deltaY: -500, canScroll: true });
    await sleep(400);
    const miss = await guestExec(tab.wc, "({scrollY,wheel:__fixture.wheel})");
    check("scroll oracle: a wheel that misses the page reads 0, as the failed run did", miss.scrollY === 0 && miss.wheel === 0, miss);

    const after = await askInputApp();
    check("no pointer moved", after.pointer[0] === before.pointer[0] && after.pointer[1] === before.pointer[1], { before: before.pointer, after: after.pointer });
    const frontChanges = watchSince().filter((row) => row.isWatched === true || (row.kind === "active" && row.active === true));
    check("the dev app never came to the front or went active", frontChanges.length === 0, { frontChanges, frontNow: after.front });
    const onScreen = watchSince().filter((row) => row.kind === "dev-window-on-user-screen");
    check("no dev window showed beyond the strip", onScreen.length === 0, { onScreen });
    await control("closeWindow", { name });
    await finish(results.every((each) => each.passed !== false) ? 0 : 1);
  } catch (error) {
    check("preflight got through", false, { error: String(error?.message ?? error) });
    await finish(1);
  }
}

try {
  const name = "go";
  const session = `session-${name}`;
  const display = (await control("info")).display;
  const made = await control("createWindow", { name, bounds: { width: 1280, height: 800 }, query: `bcw=${name}` });
  const windowId = windowIdOf(made.media);
  const bounds = { x: Math.round((display.width - 1280) / 2), y: 100, width: 1280, height: 800 };
  await control("setBounds", { name, bounds });
  await control("showAtEdge", { name, bounds });
  await sleep(5000);
  await selectSession(name, session);
  const tab = await openTab(name, session, `${FIXTURES}/page/336699/go`);
  await sleep(1000);
  const tools = pointTools(name, tab, { onScreen: true });
  const guest = (js) => guestExec(tab.wc, js);
  const snap = async (id) => (await capture(windowId, id, folder)).file;
  /** What the screen shows at a point, cursor included: an ordinary window capture leaves the cursor out. */
  const snapCursor = (id, point) => {
    const file = path.join(folder, `${id}.png`);
    execFileSync("/usr/sbin/screencapture", ["-x", "-C", "-R", `${Math.round(point.x - 90)},${Math.round(point.y - 60)},180,120`, file]);
    return file;
  };
  const video = (id, seconds) => {
    const file = path.join(folder, `${id}.mp4`);
    return { file, done: record(windowId, seconds, file) };
  };
  /** A check that throws fails loudly and the run goes on, unless a person stopped it. */
  const step = async (id, run) => {
    try {
      await run();
    } catch (error) {
      if (stopped) throw error;
      let image = null;
      try { image = await snap(`${id}-failed`); } catch { /* the error is what counts */ }
      check(id, false, { error: String(error?.message ?? error), image });
    }
  };
  const moveTo = async (point, hoverSelector) => {
    await real({ op: "move", ...point });
    await sleep(300);
    if (hoverSelector && !(await guest(`document.querySelector(${JSON.stringify(hoverSelector)}).matches(":hover")`))) throw new Error(`the pointer isn't over ${hoverSelector}`);
  };

  await step("focus", async () => {
    await real({ op: "click", ...(await tools.inApp(100, 700, "#root")) });
    await sleep(400);
    const front = await real({ op: "front" });
    const info = await control("windowInfo", { name });
    const page = await guest("document.hasFocus()");
    check("focus", front.pid === (await control("info")).pid && info.focused, { front: front.front, focused: info.focused, pageHasFocus: page, image: await snap("focus") });
  });

  await step("click-in-page", async () => {
    await guest("__fixture.clicks=0;__fixture.keys=0;addEventListener('click',(e)=>{window.__trusted=e.isTrusted});1");
    await real({ op: "click", ...(await tools.inPage("#field", "#field")) });
    await sleep(300);
    check("click-in-page", (await guest("__fixture.clicks")) === 1 && (await guest("window.__trusted")) === true && (await guest("document.activeElement.id")) === "field", { image: await snap("click-in-page") });
  });

  await step("typing", async () => {
    await real({ op: "type", text: "real keys" });
    await sleep(300);
    const value = await guest("document.getElementById('field').value");
    check("typing", value === "real keys", { value, image: await snap("typing") });
  });

  await step("address", async () => {
    const centre = await tools.centreOf('[data-pane="browser"][data-active] input[aria-label="Address"]');
    await real({ op: "click", ...(await tools.inApp(centre.x, centre.y, 'input[aria-label="Address"]')) });
    await sleep(300);
    await real({ op: "type", text: `${FIXTURES}/page/116611/go-two` });
    await real({ op: "key", code: 36 });
    await sleep(1200);
    const url = await guest("location.href");
    check("address", url.includes("go-two"), { url, image: await snap("address") });
  });

  await step("hover-cursor", async () => {
    const middle = await tools.pageMiddle();
    const read = async (id, where, selector) => {
      const point = await tools.inPage(where, selector);
      await moveTo(point, selector);
      const cursor = (await real({ op: "cursor" })).cursor;
      return { cursor, image: snapCursor(`hover-${id}`, point) };
    };
    const link = await read("link", "#link", "#link");
    const field = await read("field", "#field", "#field");
    const page = await read("page", middle, "#tall");
    check("hover-cursor", link.cursor === "hand" && field.cursor === "text" && page.cursor === "arrow", { link, field, page });
  });

  await step("scroll", async () => {
    const middle = await tools.pageMiddle();
    await guest("scrollTo(0,0);__fixture.wheel=0;1");
    await moveTo(await tools.inPage(middle, "#tall"), "#tall");
    const recording = video("scroll", 2.5);
    await sleep(300);
    for (let n = 0; n < 40; n++) {
      await real({ op: "scroll", dy: -20 });
      await sleep(16);
    }
    await recording.done;
    const read = await guest("({scrollY,wheel:__fixture.wheel})");
    check("scroll", read.scrollY > 100 && read.wheel > 0, { ...read, video: recording.file, image: await snap("scroll") });
    await guest("scrollTo(0,0);1");
  });

  await step("menu", async () => {
    await guest("__fixture.clicks=0;1");
    const recording = video("menu", 6);
    await sleep(300);
    const button = await tools.toolbarButton();
    await real({ op: "click", ...(await tools.inApp(button.x, button.y, "button")) });
    await sleep(500);
    const menu = await host(name, `(()=>{const r=document.querySelector("[role=menu]").getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}})()`);
    const opened = await snap("menu-open");
    const beside = await tools.inPage({ x: Math.round(menu.x - 200 - (await tools.pageRect()).x), y: Math.round(menu.y + 300 - (await tools.pageRect()).y) }, "html");
    await real({ op: "click", ...beside });
    await sleep(400);
    const closed = await host(name, `document.querySelector("[role=menu]") === null`);
    const clicksInPage = await guest("__fixture.clicks");
    await real({ op: "click", ...(await tools.inApp(button.x, button.y, "button")) });
    await sleep(500);
    const item = await host(name, `(()=>{const e=[...document.querySelectorAll("[role=menuitem]")].find((x)=>x.textContent.trim()==="Find in page");const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
    await real({ op: "click", ...(await tools.inApp(item.x, item.y, "[role=menuitem]")) });
    await sleep(500);
    const findBar = await host(name, `document.querySelector('input[aria-label="Find in page"]') !== null`);
    const picked = await snap("menu-picked-find");
    await real({ op: "key", code: 53 });
    await sleep(400);
    const findClosed = await host(name, `document.querySelector('input[aria-label="Find in page"]') === null`);
    await recording.done;
    check("menu", closed && clicksInPage === 0 && findBar, { closed, clicksInPage, pickedFindOpenedBar: findBar, escapeClosedIt: findClosed, video: recording.file, images: [opened, picked] });
  });

  await step("divider", async () => {
    // From a known width, so the check doesn't depend on where the last run left the sidebar or on its cap.
    await setSidebarWidth(name, 292);
    if (Math.abs((await tools.pageRect()).x - 292) > 2) throw new Error(`the sidebar didn't go to its known width: page at ${(await tools.pageRect()).x}`);
    await guest("__fixture.pointer=0;1");
    const centre = await tools.centreOf('[data-slot="sidebar-resize-handle"]');
    const before = await tools.pageRect();
    const recording = video("divider", 2.5);
    await sleep(300);
    const from = await tools.inApp(centre.x, 400, '[data-slot="sidebar-resize-handle"]');
    await moveTo(from);
    const resizeCursor = (await real({ op: "cursor" })).cursor;
    await real({ op: "drag", ...from, toX: from.x + 150, toY: from.y, steps: 60 });
    await recording.done;
    const after = await tools.pageRect();
    check("divider", after.x > before.x + 100 && (await guest("__fixture.pointer")) === 0 && resizeCursor === "resize-col", { from: before.x, to: after.x, resizeCursor, video: recording.file, image: await snap("divider") });
  });

  await step("window-drag", async () => {
    await guest("__fixture.pointer=0;1");
    const start = (await control("windowInfo", { name })).bounds;
    const before = await tools.pageRect();
    const { strip, point } = await tools.dragStrip();
    const recording = video("window-drag", 2.5);
    await sleep(300);
    await real({ op: "drag", ...point, toX: point.x + 80, toY: point.y + 40, steps: 60 });
    await recording.done;
    const moved = (await control("windowInfo", { name })).bounds;
    const after = await tools.pageRect();
    const pointer = await guest("__fixture.pointer");
    // In every frame, the same spot of the window is still the page: it stayed locked inside the window.
    const box = { x: Math.round((before.x + before.width / 2) * 2), y: Math.round((before.y + before.height / 2) * 2), width: 16, height: 16 };
    const frames = await framesOf(recording.file, box);
    const colour = (frame) => [frame[0], frame[1], frame[2]];
    const reference = colour(frames[0]);
    const offFrames = frames.filter((frame) => colour(frame).some((value, at) => Math.abs(value - reference[at]) > 30)).length;
    const pageSame = after.x === before.x && after.y === before.y && after.width === before.width && after.height === before.height;
    check("window-drag", moved.x - start.x >= 60 && moved.y - start.y >= 20 && pageSame && pointer === 0 && offFrames === 0, { strip, start, moved, pageSame, pointer, frames: frames.length, framesWithoutThePage: offFrames, video: recording.file, image: await snap("window-drag") });
    await control("setBounds", { name, bounds });
    await sleep(300);
  });

  await step("shortcuts", async () => {
    const tabsCount = () => host(name, `(async()=>(await import("/src/state/sessionTabs.ts")).sessionTabs(${JSON.stringify(session)}).tabs.length)()`);
    const count = await tabsCount();
    await real({ op: "click", ...(await tools.inApp(100, 700, "#root")) });
    await real({ op: "key", code: 17, flags: ["cmd"] });
    await sleep(400);
    const afterT = await tabsCount();
    const imageT = await snap("shortcut-cmd-t");
    await real({ op: "key", code: 11, flags: ["cmd", "shift"] });
    await sleep(400);
    const afterB = await tabsCount();
    check("shortcuts", afterT === count + 1 && afterB === count + 2, { count, afterT, afterB, images: [imageT, await snap("shortcut-cmd-shift-b")] });
    await host(name, `(async()=>{const s=await import("/src/state/sessionTabs.ts");s.selectTab(${JSON.stringify(session)},${JSON.stringify(tab.id)});return 1})()`);
    await sleep(400);
  });

  await step("minimise", async () => {
    const origin = await guest("performance.timeOrigin");
    const before = await snap("minimise-before");
    const { x, y } = (await control("windowInfo", { name })).bounds;
    await real({ op: "click", x: x + 39, y: y + 16 });
    await sleep(1500);
    const minimised = (await control("windowInfo", { name })).minimized;
    // Restored by script: finding a minimised window's tile in the Dock would take another grant.
    await control("restore", { name });
    await sleep(1500);
    await guest(`__paintNonce("ff00ff");1`);
    await sleep(400);
    const shot = await capture(windowId, "minimise-after-restore", folder);
    const band = shot.image.at(1400, 1540);
    await guest(`__paintNonce("000000");1`);
    check("minimise", minimised && (await guest("performance.timeOrigin")) === origin && band[0] > 180 && band[1] < 100, { minimised, restoredBy: "script", band, images: [before, shot.file] });
  });

  await step("another-space", async () => {
    const plan = tools.spaceStep();
    if (plan.error) throw new Error(plan.error);
    const other = await openTab(name, session, `${FIXTURES}/page/662266/other`);
    await host(name, `(async()=>{const s=await import("/src/state/sessionTabs.ts");s.selectTab(${JSON.stringify(session)},${JSON.stringify(tab.id)});return 1})()`);
    await sleep(500);
    const origin = await guest("performance.timeOrigin");
    await real({ op: "key", code: plan.go, flags: ["ctrl"] });
    awayFrom = plan;
    await sleep(2500);
    const away = spaces("list").active;
    if (away === plan.active) throw new Error(`Ctrl-${plan.direction} didn't change the Space`);
    const startedAt = Date.now();
    const read = async (label, wc = tab.wc) => ({
      label,
      seconds: Math.round((Date.now() - startedAt) / 1000),
      host: await host(name, "document.visibilityState"),
      guest: await guestExec(wc, "document.visibilityState"),
      hostCapturing: await control("hostCapturing", { name }),
    });
    const readings = [await read("quiet")];
    const beforeCapture = (await control("capturePageForRepro", { name, point: { x: 700, y: 400 } })).pixel;
    readings.push({ ...(await read("right after capturePage")), capturePage: beforeCapture });
    await host(name, `(async()=>{const s=await import("/src/state/sessionTabs.ts");s.selectTab(${JSON.stringify(session)},${JSON.stringify(other.id)});return 1})()`);
    await sleep(800);
    readings.push({ ...(await read("other tab selected while away", other.wc)), capturePage: (await control("capturePageForRepro", { name, point: { x: 700, y: 400 } })).pixel });
    readings.push(await read("other tab, after capturePage", other.wc));
    const awayImage = await snap("space-away");
    await host(name, `(async()=>{const s=await import("/src/state/sessionTabs.ts");s.selectTab(${JSON.stringify(session)},${JSON.stringify(tab.id)});return 1})()`);
    while (Date.now() - startedAt < 40000) {
      await sleep(10000);
      readings.push(await read("still away"));
    }
    const lastCapture = (await control("capturePageForRepro", { name, point: { x: 700, y: 400 } })).pixel;
    readings.push({ ...(await read("end of the 40 s")), capturePage: lastCapture });
    const recording = video("space-return", 6);
    await sleep(500);
    await real({ op: "key", code: plan.back, flags: ["ctrl"] });
    awayFrom = null;
    await sleep(2500);
    const home = spaces("list").active;
    const back = await read("back");
    await guest(`__paintNonce("ff00ff");1`);
    await sleep(400);
    const fresh = await capture(windowId, "space-back-fresh", folder);
    const band = fresh.image.at(1400, 1540);
    await recording.done;
    check("another-space", home === plan.active && back.guest === "visible" && (await guest("performance.timeOrigin")) === origin && band[0] > 180 && band[1] < 100, { direction: plan.direction, from: plan.active, away, home, readings, back, band, images: [awayImage, fresh.file], video: recording.file });
  });
  await finish(results.every((each) => each.passed) ? 0 : 1);
} catch (error) {
  console.error(String(error?.message ?? error));
  check("the run got through", false, { error: String(error?.message ?? error), stoppedBy: stopped });
  await finish(stopped?.human !== undefined ? 3 : 1);
}
