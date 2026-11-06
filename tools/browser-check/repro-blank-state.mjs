#!/usr/bin/env node
// The spike's unexplained state: every guest `hidden`, blank or stale pixels, host `visible`, renderers
// alive. Enters each way a window can stop being on screen, with and without capturePage in flight,
// and reports what host, guest, capturePage and the OS each say. The spike was the only thing that
// called capturePage; the app and the gate don't.
import fs from "node:fs";
import path from "node:path";

import { evidenceDir } from "./paths.mjs";
import { capture } from "./instruments.mjs";
import { FIXTURES, control, guestExec, host, openShell, openTab, selectSession, sleep, startCover } from "./lib.mjs";

const out = [];
const name = "repro";
const { windowId } = await openShell(name, { level: null, allSpaces: false });
const session = `session-${name}`;
await selectSession(name, session);
const tab = await openTab(name, session, `${FIXTURES}/page/990033/repro`);
await sleep(1200);
const info = await control("info");
const point = { x: 700, y: 400 };
const folder = path.join(evidenceDir, "blank-state");
fs.mkdirSync(folder, { recursive: true });

const read = async (label, { withCapture }) => {
  let captured = null;
  if (withCapture) {
    try { captured = (await control("capturePageForRepro", { name, point })).pixel; } catch (error) { captured = String(error.message).slice(0, 60); }
  }
  const row = {
    label,
    host: await host(name, "document.visibilityState"),
    guest: await guestExec(tab.wc, "document.visibilityState"),
    raf: await guestExec(tab.wc, "__fixture.raf"),
    hostCapturing: await control("hostCapturing", { name }),
    capturePagePixel: captured,
  };
  out.push(row);
  console.log(JSON.stringify(row));
  return row;
};

const enter = {
  cover: async () => { const cover = startCover(info.display.width - 40, 0, 40, info.display.height - 33); await sleep(10); return async () => cover.kill(); },
  minimise: async () => { await control("minimize", { name }); return async () => control("restore", { name }); },
  appHidden: async () => { await control("hideApp"); return async () => control("showApp"); },
};

for (const [state, go] of Object.entries(enter)) {
  for (const mode of ["none", "capture-before", "capture-during-transition"]) {
    await read(`${state}/${mode}: start`, { withCapture: false });
    let burst = null;
    if (mode === "capture-during-transition") {
      let running = true;
      burst = (async () => { while (running) { await control("capturePageForRepro", { name, point }).catch(() => {}); } })();
      burst.stop = () => { running = false; };
    }
    const leave = await go();
    await sleep(2500);
    burst?.stop();
    await burst;
    await read(`${state}/${mode}: in state, no capture just now`, { withCapture: false });
    await read(`${state}/${mode}: in state, right after capture`, { withCapture: true });
    await sleep(5000);
    await read(`${state}/${mode}: in state, 5 s after capture`, { withCapture: false });
    await guestExec(tab.wc, `__paintNonce("00ffff");1`);
    await leave();
    await sleep(1500);
    await read(`${state}/${mode}: back`, { withCapture: false });
    const shot = await capture(windowId, `${state}-${mode}-back`, folder);
    out.push({ label: `${state}/${mode}: OS after return`, band: shot.image.at(1400, 1540), page: shot.image.at(1400, 800) });
    await guestExec(tab.wc, `__paintNonce("000000");1`);
    await sleep(300);
  }
}
fs.writeFileSync(path.join(folder, "matrix.json"), JSON.stringify(out, null, 1));
process.exit(0);
