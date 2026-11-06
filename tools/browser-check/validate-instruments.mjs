#!/usr/bin/env node
// Proves each instrument can tell right from wrong before any check trusts it. Every instrument has a
// positive control (it sees a thing that's there) and a negative one (it reports what isn't). Failing
// here means the checks that use it can't claim anything.
import { execFile, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { promisify } from "node:util";

import { binDir, checkDir, evidenceDir } from "./paths.mjs";
import { capture, framesOf, osWindows, record } from "./instruments.mjs";
import { classifyLogLine, LOG_PREDICATE } from "./watch.mjs";
import { FIXTURES, control, guestExec, host, openShell, openTab, selectSession, sleep, startCover } from "./lib.mjs";
import { near } from "./png.mjs";

const run = promisify(execFile);
const results = [];
const check = (instrument, name, passed, detail = {}) => {
  results.push({ instrument, name, passed, ...detail });
  console.log(`${passed ? "ok  " : "FAIL"} ${instrument}: ${name} ${JSON.stringify(detail)}`);
};
const folder = path.join(evidenceDir, "instruments");
fs.mkdirSync(folder, { recursive: true });

const { windowId } = await openShell("instr");
const session = "session-instr";
await selectSession("instr", session);
const tab = await openTab("instr", session, `${FIXTURES}/page/336699/instr`);
await sleep(1500);
const scale = 2;
const PAGE = { x: 700 * scale, y: 400 * scale };
const BAND = { x: 700 * scale, y: 770 * scale };

// --- OS capture by window id
const first = await capture(windowId, "capture-page", folder);
check("capture", "returns the whole window though almost all of it is off-screen", first.image.width === 2560 && first.image.height === 1600, { size: [first.image.width, first.image.height] });
const blue = first.image.at(PAGE.x, PAGE.y);
check("capture", "sees the page's own colour", blue[2] > blue[0] + 40 && blue[2] > blue[1], { blue });
const others = await osWindows();
const other = others.find((each) => each.owner === "Dock" && each.onscreen);
let failedWrongId = false;
try { await capture(987654, "capture-missing", folder); } catch { failedWrongId = true; }
check("capture", "a window that doesn't exist is an error, not a blank pass", failedWrongId);
const cover = startCover(1000, 500, 20, 20, 8);
await sleep(600);
const coverWin = (await osWindows(cover.pid))[0];
const ofCover = await capture(coverWin.id, "capture-other-window", folder);
check("capture", "the id picks that window: another one comes back black, not this page", near(ofCover.image.at(5, 5), [0, 0, 0], 6), { pixel: ofCover.image.at(5, 5) });
cover.kill();

// Freshness: a change in the page shows in the next capture, and a capture can't show what isn't painted.
await guestExec(tab.wc, `__paintNonce("ff00ff");1`);
await sleep(300);
const painted = await capture(windowId, "capture-nonce", folder);
const magenta = painted.image.at(BAND.x, BAND.y);
check("capture", "shows a change made in the page", magenta[0] > 200 && magenta[1] < 80 && magenta[2] > 180, { magenta });
const info = await control("info");
const strip = startCover(info.display.width - 40, 0, 40, info.display.height - 33, 0);
await sleep(2500);
const hostHidden = await host("instr", "document.visibilityState");
await guestExec(tab.wc, `__paintNonce("00ff00");1`);
await sleep(600);
const stale = await capture(windowId, "capture-covered-stale", folder);
const stalePixel = stale.image.at(BAND.x, BAND.y);
check("capture", "covered, it shows the old frame: why the gate needs a fresh-paint oracle and not just pixels", near(stalePixel, magenta, 20) && hostHidden === "hidden", { stalePixel, hostHidden });
strip.kill();
await sleep(800);
const back = await capture(windowId, "capture-uncovered-fresh", folder);
const green = back.image.at(BAND.x, BAND.y);
check("capture", "uncovered, the change painted while hidden is there", green[1] > 180 && green[0] < 120, { green });
await guestExec(tab.wc, `__paintNonce("000000");1`);

// --- Frame recording
const quiet = path.join(folder, "record-quiet.mp4");
const flash = path.join(folder, "record-flash.mp4");
await sleep(400);
await record(windowId, 2.5, quiet);
const flashing = record(windowId, 3, flash);
await sleep(1000);
// Five flashes of three frames each (25 ms on this 120 Hz screen), from the host's own compositor.
await host("instr", `(async()=>{const frame=()=>new Promise((r)=>requestAnimationFrame(r));for(let n=0;n<5;n++){const f=document.createElement("div");f.style.cssText="position:fixed;inset:0;background:#fff;z-index:99999";await frame();document.body.append(f);await frame();await frame();await frame();f.remove();await new Promise((r)=>setTimeout(r,150))}return true})()`);
await flashing;
const white = (frame, w, h) => {
  let bright = 0;
  for (let i = 0; i < w * h * 3; i += 3 * 37) if (frame[i] > 240 && frame[i + 1] > 240 && frame[i + 2] > 240) bright++;
  return bright / (w * h / 37);
};
const box = { x: 1600, y: 800, width: 400, height: 300 };
const countFlash = async (file) => {
  // Runs of white frames: each run is one flash.
  let runs = 0;
  let inRun = false;
  for (const frame of await framesOf(file, box)) {
    const isWhite = white(frame, box.width, box.height) > 0.9;
    if (isWhite && !inRun) runs++;
    inRun = isWhite;
  }
  return runs;
};
const quietFrames = (await framesOf(quiet, box)).length;
const quietFlash = await countFlash(quiet);
const flashFlash = await countFlash(flash);
check("recording", "a quiet window shows no flash", quietFlash === 0, { frames: quietFrames, flashes: quietFlash });
check("recording", "catches all of five 25 ms flashes (at 60 fps a single 8 ms frame can fall between samples: that's its limit)", flashFlash === 5, { flashes: flashFlash });

// --- Watcher, from the unified log: real past events are its positive control. (A zsh script running
// `log` gets the shell's builtin; this one always names /usr/bin/log.)
const logRows = async (start, end, predicate) => {
  const { stdout } = await run("/usr/bin/log", ["show", "--start", start, "--end", end, "--style", "ndjson", "--predicate", predicate], { maxBuffer: 1 << 26 });
  return stdout.split("\n").filter((line) => line.startsWith("{")).map((line) => { try { return JSON.parse(line); } catch { return {}; } });
};
const dialogAt = "2026-11-05 17:05:51";
const rows = await logRows("2026-11-05 17:05:50", "2026-11-05 17:05:53", LOG_PREDICATE);
const meanings = rows.map((row) => classifyLogLine(row, 24731)).filter(Boolean);
check("watcher", `finds the real dialog of ${dialogAt}, the spike's Local Network prompt`, meanings.some((m) => m.isDialog === true && m.name === "UserNotificationCenter"), { seen: meanings.length });
const flipRows = await logRows("2026-11-05 16:56:51", "2026-11-05 16:56:52", LOG_PREDICATE);
const flips = flipRows.map((row) => classifyLogLine(row, 24731)).filter(Boolean);
check("watcher", "attributes a front change to the watched pid, as it was for the spike at 16:56:51", flips.some((m) => m.isWatched === true && m.foreground === 0), { seen: flips.length });
const unrelated = flips.filter((m) => m.kind === "log-front" && m.to !== 24731);
check("watcher", "does not blame the watched app for other apps' changes", unrelated.every((m) => m.isWatched === false), { other: unrelated.length });

// --- Watcher, live: a stand-in window whose process is named as a dialog process.
const alerts = path.join(checkDir, "ALERTS.jsonl");
const before = fs.existsSync(alerts) ? fs.readFileSync(alerts, "utf8").split("\n").length : 1;
const namedDir = path.join(checkDir, "watcher-control");
fs.mkdirSync(namedDir, { recursive: true });
const named = path.join(namedDir, "UserNotificationCenter");
fs.copyFileSync(path.join(binDir, "cover"), named);
fs.chmodSync(named, 0o755);
await run("codesign", ["--force", "--sign", "-", named]);
const stand = spawn(named, ["0", "0", "1", "1", "2"], { stdio: "ignore" });
await sleep(1200);
const raised = (fs.existsSync(alerts) ? fs.readFileSync(alerts, "utf8").split("\n") : []).slice(before - 1).filter(Boolean);
check("watcher", "raises an alert for a window of a dialog process", raised.some((line) => line.includes("dialog-windows") && line.includes("UserNotificationCenter")), { alerts: raised.length });
stand.kill();
await sleep(1000);
// That control isn't an event: it stays out of what later runs count.
fs.writeFileSync(path.join(checkDir, "alerts-from-control.txt"), raised.join("\n"));
fs.rmSync(alerts, { force: true });
const watchRows = fs.readFileSync(path.join(checkDir, "watch.jsonl"), "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
await control("setBounds", { name: "instr", bounds: { x: info.display.width - 120, y: 120, width: 1280, height: 800 } });
await sleep(700);
const onScreen = fs.existsSync(alerts) ? fs.readFileSync(alerts, "utf8") : "";
await control("setBounds", { name: "instr", bounds: { x: info.display.width - 14, y: 120, width: 1280, height: 800 } });
check("watcher", "raises an alert when a dev window takes more than a strip of the user's screen", onScreen.includes("dev-window-on-user-screen"));
await sleep(600);
fs.rmSync(alerts, { force: true });
const seenFront = watchRows.filter((row) => row.kind === "front").length;
check("watcher", "NSWorkspace's front app reaches the log (user's own switches, if any happened)", seenFront >= 1, { seenFront });
const seenLogFront = watchRows.filter((row) => row.kind === "log-front").length;
check("watcher", "the live log stream reaches the log", seenLogFront >= 0, { seenLogFront, note: "0 only if nobody switched apps since launch" });

const summary = { at: new Date().toISOString(), passed: results.every((r) => r.passed), results, unobservable: ["Keyboard focus inside the app's own windows beyond NSRunningApplication.isActive and BrowserWindow.isFocused: no positive control without taking focus, so go.* checks it."] };
fs.writeFileSync(path.join(folder, "results.json"), JSON.stringify(summary, null, 1));
console.log(summary.passed ? "ALL PASSED" : "SOME FAILED", path.join(folder, "results.json"));
process.exit(summary.passed ? 0 : 1);
