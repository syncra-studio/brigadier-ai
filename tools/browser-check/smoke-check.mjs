#!/usr/bin/env node
// A launch smoke check of the production page, with no driver: the app on stock Electron with --smoke, the
// addon and daemon as built, parked and unfocused, on scratch data. Run a first launch (new data folder) and
// then a cold one. Hold the instance lock, and have no other instance up. Real startup and real daemon;
// the page's own measurements. Build with `build-native.mjs --release` for numbers worth comparing.
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { addonPath, appBinary, binDir, checkDir, daemonHome, desktopDir, sidecarDir } from "./paths.mjs";

const data = path.join(checkDir, "smoke-data");
const chromium = path.join(checkDir, "smoke-chromium");
const reports = path.join(checkDir, "evidence/smoke");
const runs = Number(process.argv[2] ?? 1);
fs.mkdirSync(reports, { recursive: true });
fs.mkdirSync(daemonHome, { recursive: true });
fs.rmSync(data, { recursive: true, force: true });
fs.rmSync(chromium, { recursive: true, force: true });
fs.mkdirSync(chromium, { recursive: true });

function launch(name) {
  const report = path.join(reports, `${name}.json`);
  fs.rmSync(report, { force: true });
  const env = {
    ...process.env,
    BRIGADIER_DEV: "1",
    BRIGADIER_UNFOCUSED: "1",
    BRIGADIER_DEV_DATA: chromium,
    BRIGADIER_ADDON: addonPath,
    BRIGADIER_PAGE_DIR: path.join(desktopDir, "dist-electron/page"),
    BRIGADIER_DEV_DAEMON_DATA: data,
    BRIGADIER_DEV_SIDECARS: sidecarDir,
    BRIGADIER_DEV_DAEMON_HOME: daemonHome,
    BRIGADIER_CHECK_BOUNDS: JSON.stringify({ x: 1714, y: 120, width: 1280, height: 800 }),
    BRIGADIER_SMOKE_REPORT: report,
    BRIGADIER_LOG: "warn",
  };
  return new Promise((resolve) => {
    const child = spawn(path.join(binDir, "spawn-disclaimed"), [appBinary, "--smoke"], { env, stdio: ["ignore", "ignore", "pipe"] });
    let log = "";
    child.stderr.on("data", (chunk) => { log += chunk; });
    child.on("exit", (code) => resolve({ code, log, report }));
  });
}

for (let run = 0; run < runs + 1; run++) {
  const name = run === 0 ? "first" : `cold-${run}`;
  const { code, log, report } = await launch(name);
  if (!fs.existsSync(report)) {
    console.log(`${name}: exit ${code}, no report\n${log.slice(-1500)}`);
    process.exit(1);
  }
  const parsed = JSON.parse(fs.readFileSync(report, "utf8"));
  const failing = (parsed.checks ?? []).filter((check) => check.status === "fail");
  const cold = (parsed.checks ?? []).find((check) => /old start/.test(check.metric) && check.measured !== null);
  const rss = (parsed.checks ?? []).find((check) => check.id === "coreIdleRss");
  console.log(`${name}: exit ${code}, passed ${parsed.passed}, ${cold?.metric} ${Math.round(cold?.measured)} ms (limit ${cold?.limit}), daemon idle ${rss ? Math.round(rss.measured) : "?"} MB`);
  for (const check of failing) console.log(`  FAIL ${check.metric}: ${check.measured} ${check.note ? `(${String(check.note).slice(0, 200)})` : ""}`);
}
process.exit(0);
