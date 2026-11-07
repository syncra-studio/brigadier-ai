// Shared steps for the checks: drive the app through the real front end's own functions.
import { execFileSync, spawn } from "node:child_process";
import path from "node:path";

import { call } from "./bc.mjs";
import { binDir } from "./paths.mjs";
import { windowIdOf } from "./instruments.mjs";

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export const control = (name, args = {}, timeout) => call({ op: "control", name, args }, timeout);
export const host = (window, js, timeout) => call({ op: "host", window, js }, timeout);
export const FIXTURES = `http://127.0.0.1:${process.env.BRIGADIER_FIXTURE_PORT ?? 1432}`;

/** Off-screen but for a thin strip at the right edge, so the app stays out of the user's way. */
export function parkedBounds(display, index = 0) {
  return { x: display.width - 14, y: 120 + index * 60, width: 1280, height: 800 };
}

/** Makes a window of the real front end and waits until its app has started. */
export async function openShell(name, { index = 0, level = "floating", allSpaces = true, bounds, real = false } = {}) {
  const info = await control("info");
  const park = bounds ? { ...parkedBounds(info.display, index), ...bounds(info.display) } : parkedBounds(info.display, index);
  const made = info.windows.includes(name) ? await control("windowInfo", { name }) : await control("createWindow", { name, bounds: { width: park.width, height: park.height }, query: `bcw=${name}`, real });
  // Electron clamps a position given at creation to the screen, and macOS may clamp it again at first
  // show, so it's placed and checked around the show: a window on the user's screen is a failed run.
  await control("setBounds", { name, bounds: park });
  if (level) await control("setLevel", { name, level });
  if (allSpaces) await control("allSpaces", { name, on: true });
  await control("showAtEdge", { name, bounds: park });
  const placed = (await control("windowInfo", { name })).bounds;
  if (placed.x !== park.x) {
    await control("closeWindow", { name });
    throw new Error(`${name}: not parked at the edge, at x=${placed.x}: closed it`);
  }
  for (let tries = 0; ; tries++) {
    const ready = await host(name, `!document.getElementById("splash") && !!document.querySelector("[data-slot]")`);
    if (ready) break;
    if (tries > 120) throw new Error(`${name}: the app did not start`);
    await sleep(250);
  }
  return { name, windowId: windowIdOf(made.media ?? (await control("windowInfo", { name })).media) };
}

export const selectSession = (window, session) =>
  host(window, `(async()=>{const a=await import("/src/state/actions.ts");await a.openConversation(${JSON.stringify(session)});return true})()`);

/** Opens a browser tab by the app's own `openBrowserTab`, waits for its page; its tab id. */
export async function openTab(window, session, url) {
  const id = await host(window, `(async()=>{const s=await import("/src/state/sessionTabs.ts");return s.openBrowserTab(${JSON.stringify(session)},${JSON.stringify(url)})})()`);
  for (let tries = 0; ; tries++) {
    const state = await layerState(window);
    const page = state.pages.find((each) => each.id === id);
    if (page?.wc) return { id, wc: page.wc };
    if (tries > 200) throw new Error(`${url}: the page did not open`);
    await sleep(100);
  }
}

export const selectTab = (window, session, id) =>
  host(window, `(async()=>{const s=await import("/src/state/sessionTabs.ts");s.selectTab(${JSON.stringify(session)},${JSON.stringify(id)});return true})()`);

export const layerState = (window) =>
  host(window, "window.__brigadierTabLayer.layerState()");

export const guestExec = (id, js) => control("guestExec", { id, js });

/** A real window of another process over a part of the screen, for as long as it runs. */
export function startCover(x, yFromBottom, width, height, seconds = 0) {
  return spawn(path.join(binDir, "cover"), [x, yFromBottom, width, height, seconds].map(String), { stdio: "ignore" });
}

export function spaces(...args) {
  return JSON.parse(execFileSync(path.join(binDir, "spaces"), args.map(String)).toString());
}

/** Puts the sidebar at a known width through the app's own divider, by script: a run starts from where the last left it. */
export async function setSidebarWidth(window, width) {
  const handle = await host(window, `(()=>{const r=document.querySelector('[data-slot="sidebar-resize-handle"]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+300}})()`);
  const y = Math.round(handle.y);
  const send = (event) => control("hostSendInput", { name: window, event });
  await send({ type: "mouseMove", x: Math.round(handle.x), y });
  await send({ type: "mouseDown", x: Math.round(handle.x), y, button: "left", clickCount: 1 });
  await send({ type: "mouseMove", x: width, y, modifiers: ["leftButtonDown"] });
  await sleep(60);
  await send({ type: "mouseUp", x: width, y, button: "left", clickCount: 1 });
  await sleep(200);
}
