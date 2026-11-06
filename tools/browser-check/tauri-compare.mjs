#!/usr/bin/env node
// The Tauri build's numbers, for comparing with Electron's, without taking focus: a build whose window config
// parks it on the strip (`tauri build --config`), made background-only here. Launch smoke checks (first, then cold),
// then one idle launch for memory and a window capture. Scratch HOME and data. Hold the instance lock.
//   tauri-compare.mjs <Brigadier Dev.app> [cold runs]
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { osWindows, capture } from "./instruments.mjs";
import { binDir, checkDir, daemonHome, evidenceDir } from "./paths.mjs";

const app = path.resolve(process.argv[2]);
const runs = Number(process.argv[3] ?? 3);
const exe = path.join(app, "Contents/MacOS/brigadier-app");
const data = path.join(checkDir, "tauri-data");
const reports = path.join(evidenceDir, "tauri");
const load = () => `load ${os.loadavg().map((value) => value.toFixed(1)).join(" ")}`;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
fs.mkdirSync(reports, { recursive: true });
fs.mkdirSync(daemonHome, { recursive: true });
fs.rmSync(data, { recursive: true, force: true });
fs.mkdirSync(data, { recursive: true });
execFileSync("plutil", ["-replace", "LSBackgroundOnly", "-bool", "true", path.join(app, "Contents/Info.plist")]);
execFileSync("codesign", ["--force", "--deep", "--sign", "-", app]);

const env = { ...process.env, HOME: daemonHome, BRIGADIER_BACKGROUND_LAUNCH: "1", BRIGADIER_LOG: "warn" };
const launch = (args, extra = {}) => spawn(path.join(binDir, "spawn-disclaimed"), [exe, "--brigadier-data-dir", data, ...args], { env: { ...env, ...extra }, stdio: ["ignore", "ignore", "pipe"] });

for (let run = 0; run <= runs; run++) {
  const name = run === 0 ? "first" : `cold-${run}`;
  const report = path.join(reports, `${name}.json`);
  fs.rmSync(report, { force: true });
  const loadBefore = load();
  const child = launch(["--smoke"], { BRIGADIER_SMOKE_REPORT: report });
  let log = "";
  child.stderr.on("data", (chunk) => { log += chunk; });
  const code = await new Promise((resolve) => child.on("exit", resolve));
  if (!fs.existsSync(report)) {
    console.log(`${name}: exit ${code}, no report\n${log.slice(-1200)}`);
    process.exit(1);
  }
  const parsed = JSON.parse(fs.readFileSync(report, "utf8"));
  const start = parsed.checks.find((check) => ["coldStart", "firstColdStart"].includes(check.id) && check.measured !== null);
  console.log(`${name}: exit ${code}, passed ${parsed.passed}, ${start.metric} ${Math.round(start.measured)} ms (limit ${start.limit}): ${String(start.note).split("since process start: ")[1]}; before: ${loadBefore}; after: ${load()}`);
}

// One idle launch.
const launchedAt = Date.now();
const child = launch([]);
await sleep(15000);
const rows = execFileSync("ps", ["-axo", "pid=,rss=,lstart=,command="], { encoding: "utf8" }).trim().split("\n").map((line) => {
  const match = /^\s*(\d+)\s+(\d+)\s+(\w+ \w+\s+\d+ [\d:]+ \d+)\s+(.*)$/.exec(line);
  return match ? { pid: Number(match[1]), mb: Number(match[2]) / 1024, started: new Date(match[3]).getTime(), command: match[4] } : null;
}).filter(Boolean);
const ours = rows.filter((row) => row.command.startsWith(app) || (row.command.includes("com.apple.WebKit") && row.started >= launchedAt - 1000));
console.log(`Tauri processes, idle after 15 s: ${ours.map((row) => `${path.basename(row.command.split(" ")[0])} ${Math.round(row.mb)}`).join(", ")}; total without the daemon ${Math.round(ours.filter((row) => !row.command.includes("brigadierd")).reduce((sum, row) => sum + row.mb, 0))} MB; ${load()}`);
const windows = await osWindows(child.pid);
const window = windows.find((each) => each.width >= 800 || each.w >= 800) ?? windows[0];
console.log(`windows: ${JSON.stringify(windows).slice(0, 300)}`);
if (window?.id) await capture(window.id, "idle", reports);
for (const row of ours) {
  if (row.command.includes("brigadierd") || row.command.startsWith(app)) {
    try { process.kill(row.pid, "SIGTERM"); } catch { /* gone */ }
  }
}
await sleep(1500);
process.exit(0);
