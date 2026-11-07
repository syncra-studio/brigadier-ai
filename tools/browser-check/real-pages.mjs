#!/usr/bin/env node
// Real public pages in the real app's <webview> on this OS, the sandbox read from each process by the OS's own means,
// H.264 and AAC playback from a local file, and how a cookie lands on disk. Real engine and network; scripted, with no
// real input: the guest is made by script, not through the address bar.
//   real-pages.mjs --package <app folder or Brigadier.app>   CI: launches it itself and quits it (needs a screen)
//   real-pages.mjs --kept-alive                              this Mac: the instance devd.mjs keeps (hold the lock)
// Writes real-pages.json, real-pages.md and page screenshots to $BRIGADIER_CHECK_OUT (default: a temp folder).
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

import { sandboxReader } from "./sandbox-state.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const out = process.env.BRIGADIER_CHECK_OUT ?? fs.mkdtempSync(path.join(os.tmpdir(), "real-pages-"));
fs.mkdirSync(out, { recursive: true });
const PROFILE = "real-pages-check";
const PARTITION = `persist:browser-profile-${PROFILE}`;
const PAGES = ["https://example.com/", "https://en.wikipedia.org/wiki/Chromium_(web_browser)", "https://github.com/", "https://www.mozilla.org/en-US/", "https://news.ycombinator.com/"];
const failures = [];
const expect = (what, passed, detail = "") => {
  console.log(`${passed ? "ok  " : "FAIL"} ${what}${detail ? `: ${detail}` : ""}`);
  if (!passed) failures.push(what);
};

/** Launches a packaged check build on the debugging pipe, as devd.mjs does on this Mac, with scratch data. */
async function launchPackage(where) {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "real-pages-data-"));
  const mac = where.endsWith(".app");
  const binary = mac ? path.join(where, "Contents/MacOS/brigadier-app") : path.join(where, process.platform === "win32" ? "brigadier-app.exe" : "brigadier-app");
  const resources = mac ? path.join(where, "Contents/Resources") : path.join(where, "resources");
  const data = path.join(scratch, "chromium");
  const env = {
    ...process.env,
    BRIGADIER_DEV: "1",
    BRIGADIER_DEV_DATA: data,
    BRIGADIER_PAGE_DIR: path.join(resources, "app.asar/page"),
    BRIGADIER_CONTROL_HTML: path.join(here, "../../apps/desktop/electron/control.html"),
    BRIGADIER_DEV_DAEMON_DATA: path.join(scratch, "daemon"),
    BRIGADIER_DEV_DAEMON_HOME: path.join(scratch, "home"),
    BRIGADIER_LOG: "warn",
  };
  fs.mkdirSync(env.BRIGADIER_DEV_DAEMON_HOME, { recursive: true });
  const app = spawn(binary, ["--remote-debugging-pipe"], { env, stdio: ["ignore", "inherit", "inherit", "pipe", "pipe"] });
  const exited = new Promise((resolve) => app.on("exit", resolve));
  let id = 1;
  const waiting = new Map();
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const at = id++;
    waiting.set(at, { resolve, reject, method });
    app.stdio[3].write(`${JSON.stringify({ id: at, method, params, ...(sessionId ? { sessionId } : {}) })}\0`);
  });
  let buffer = "";
  app.stdio[4].on("data", (chunk) => {
    buffer += chunk.toString("utf8");
    for (let at; (at = buffer.indexOf("\0")) >= 0; buffer = buffer.slice(at + 1)) {
      const message = JSON.parse(buffer.slice(0, at));
      const waiter = waiting.get(message.id);
      if (!waiter) continue;
      waiting.delete(message.id);
      if (message.error) waiter.reject(new Error(`${waiter.method}: ${message.error.message}`));
      else waiter.resolve(message.result);
    }
  });
  void exited.then((code) => { for (const waiter of waiting.values()) waiter.reject(new Error(`the app exited (${code})`)); });
  const attach = async (match) => {
    for (let tries = 0; tries < 300; tries++) {
      const target = (await send("Target.getTargets")).targetInfos.find(match);
      if (target) return (await send("Target.attachToTarget", { targetId: target.targetId, flatten: true })).sessionId;
      await sleep(100);
    }
    throw new Error("no such target");
  };
  const evaluate = async (session, expression) => {
    const { result, exceptionDetails } = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, session);
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
    return result.value;
  };
  const controlPage = await attach((target) => target.url.endsWith("control.html"));
  while (!(await evaluate(controlPage, "Boolean(window.brigadierShell)"))) await sleep(50);
  const control = (name, options = {}) => evaluate(controlPage, `window.brigadierShell.devControl(${JSON.stringify(name)}, ${JSON.stringify(options)})`);
  let page = null;
  return {
    data,
    control,
    host: async (js) => evaluate((page ??= await attach((target) => target.type === "page" && target.url.includes("bcw=app"))), js),
    open: async () => {
      const bounds = { x: 40, y: 40, width: 1280, height: 800 };
      await control("createWindow", { name: "app", bounds: { width: bounds.width, height: bounds.height }, query: "bcw=app", real: true });
      await control("showAtEdge", { name: "app", bounds });
    },
    finish: async () => {
      await control("quit").catch(() => {});
      const code = await Promise.race([exited, sleep(30000).then(() => "still running after 30 s")]);
      console.log(`app quit: ${code}`);
      expect("the app quits in order", code === 0, String(code));
    },
  };
}

/** The kept-alive instance on this Mac, parked and unfocused by lib.mjs. Its data stays up: `finish` leaves it running. */
async function keptAlive() {
  const { control, host, openShell } = await import("./lib.mjs");
  const { dataDir } = await import("./paths.mjs");
  return { data: dataDir, control, host: (js) => host("app", js), open: () => openShell("app", { real: true }), finish: async () => {} };
}

const driver = args.includes("--kept-alive") ? await keptAlive() : await launchPackage(args[args.indexOf("--package") + 1]);
const { control, host } = driver;

// The local file, and a made-up cookie to follow to disk. Ranges, as a media element asks for them.
const video = fs.readFileSync(path.join(here, "fixtures/h264-aac.mp4"));
const server = http.createServer((request, response) => {
  if (request.url === "/h264-aac.mp4") {
    const range = /bytes=(\d+)-(\d*)/.exec(request.headers.range ?? "");
    const start = range ? Number(range[1]) : 0;
    const end = range?.[2] ? Number(range[2]) : video.length - 1;
    response.writeHead(range ? 206 : 200, { "Content-Type": "video/mp4", "Accept-Ranges": "bytes", "Content-Length": end - start + 1, ...(range ? { "Content-Range": `bytes ${start}-${end}/${video.length}` } : {}) });
    response.end(video.subarray(start, end + 1));
  } else {
    response.writeHead(200, { "Content-Type": "text/html", "Set-Cookie": "brigadier_check=made-up-cookie-value; Max-Age=86400; HttpOnly" });
    response.end(`<!doctype html><title>local video</title><video id="v" src="/h264-aac.mp4" muted playsinline loop width="160" height="120"></video>`);
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const local = `http://127.0.0.1:${server.address().port}/`;

/** One <webview> under the page's policy, in the page's own stack; its web contents once the main frame has finished or failed. */
async function openGuest(url) {
  const before = new Set((await control("guests")).map((guest) => guest.id));
  // As a tab does: main attaches a guest only to a partition it was asked for.
  await host(`window.brigadierShell.profilePartition(${JSON.stringify(PROFILE)})`);
  await host(`(()=>{const w=document.createElement("webview");w.id="real-guest";window.__realGuest={};w.addEventListener("did-fail-load",(e)=>{if(e.isMainFrame&&e.errorCode!==-3)window.__realGuest.failed=e.errorCode+" "+e.errorDescription});w.addEventListener("render-process-gone",(e)=>{window.__realGuest.gone=e.reason});w.addEventListener("did-finish-load",()=>{window.__realGuest.finished=true});w.setAttribute("partition",${JSON.stringify(PARTITION)});w.src=${JSON.stringify(url)};w.style.cssText="position:fixed;left:0;top:0;width:1200px;height:760px;z-index:99999";document.body.appendChild(w);return true})()`);
  for (let tries = 0; tries < 600; tries++) {
    await sleep(100);
    const state = await host("window.__realGuest");
    if (state.failed || state.gone) return { state };
    if (state.finished) {
      const guest = (await control("guests")).find((each) => !before.has(each.id));
      if (guest) return { guest, state };
    }
  }
  return { state: { failed: "not finished after 60 s" } };
}
const closeGuest = () => host(`document.getElementById("real-guest").remove(),true`);

await driver.open();
const report = { platform: process.platform, arch: process.arch, info: await control("info"), pages: [], sandbox: [], video: null, cookie: null, keychain: null };
console.log(`Electron ${report.info.electron}, Chromium ${report.info.chrome}, ${process.platform} ${process.arch}`);
for (let tries = 0; tries < 240 && !(await host(`document.readyState==="complete"`)); tries++) await sleep(250);

const readSandbox = sandboxReader(out);
function readAll(metrics, guests) {
  const rows = [];
  for (const { pid, type, sandboxed, service } of metrics) {
    let os_;
    try { os_ = readSandbox(pid); } catch (error) { os_ = { sandboxed: null, detail: `could not read: ${String(error.message).split("\n")[0]}` }; }
    rows.push({ pid, type, service: service ?? "", guest: guests.some((guest) => guest.pid === pid), chromiumSays: sandboxed ?? null, osSays: os_.sandboxed, detail: os_.detail });
  }
  return rows;
}

for (const url of PAGES) {
  const { guest, state } = await openGuest(url);
  const row = { url, finalUrl: guest?.url ?? null, failed: state.failed ?? state.gone ?? null };
  if (guest) {
    Object.assign(row, await control("guestExec", { id: guest.id, js: `({title:document.title,text:document.body?.innerText.length??0,webdriver:navigator.webdriver})` }));
    const shot = await control("guestCapture", { id: guest.id });
    row.colours = shot.colours;
    fs.writeFileSync(path.join(out, `page-${new URL(url).hostname}.png`), Buffer.from(shot.png, "base64"));
    if (url === PAGES[0]) report.sandbox = readAll(await control("metrics"), await control("guests"));
  }
  report.pages.push(row);
  expect(`${url} loaded and painted`, Boolean(guest) && !row.failed && row.text > 0 && row.colours > 1, row.failed ?? `"${row.title}", ${row.text} characters, ${row.colours} colours sampled`);
  await closeGuest();
}

const guestRows = report.sandbox.filter((row) => row.guest);
const browserRow = report.sandbox.find((row) => row.type === "Browser");
expect("negative control: the browser process reads as not sandboxed", browserRow?.osSays === false, browserRow?.detail);
expect("the page's guest process was read", guestRows.length > 0);
for (const row of report.sandbox.filter((each) => each.type === "Tab")) expect(`renderer ${row.pid}${row.guest ? " (the web page)" : " (the app)"} is sandboxed, by the OS`, row.osSays === true, row.detail);
for (const row of report.sandbox.filter((each) => each.type !== "Tab" && each.type !== "Browser")) console.log(`     ${row.type} ${row.service} ${row.pid}: OS says ${row.osSays} (${row.detail}); Chromium says ${row.chromiumSays}`);

const { guest: player, state: playerState } = await openGuest(local);
if (player) {
  report.video = await control("guestExec", { id: player.id, js: `(async()=>{
    const v=document.getElementById("v");
    const types={h264:v.canPlayType('video/mp4; codecs="avc1.42E01E"'),aac:v.canPlayType('audio/mp4; codecs="mp4a.40.2"'),bogus:v.canPlayType('video/mp4; codecs="bogus.1"')};
    const capability=await navigator.mediaCapabilities.decodingInfo({type:"file",video:{contentType:'video/mp4; codecs="avc1.42E01E"',width:160,height:120,bitrate:100000,framerate:15}});
    await v.play();
    await new Promise((resolve)=>setTimeout(resolve,1500));
    const c=document.createElement("canvas");c.width=160;c.height=120;const x=c.getContext("2d");x.drawImage(v,0,0);
    const seen=new Set();const d=x.getImageData(0,0,160,120).data;for(let i=0;i<d.length;i+=4)seen.add((d[i]>>4)+"-"+(d[i+1]>>4)+"-"+(d[i+2]>>4));
    return {types,capability:{supported:capability.supported,smooth:capability.smooth,powerEfficient:capability.powerEfficient},time:v.currentTime,error:v.error?.message??null,width:v.videoWidth,frames:v.getVideoPlaybackQuality().totalVideoFrames,audioBytes:v.webkitAudioDecodedByteCount,colours:seen.size};
  })()` });
  const shown = report.video;
  console.log(`video: ${JSON.stringify(shown)}`);
  expect("H.264 plays: time advances, frames decoded, picture has colour", shown.time > 0 && shown.width === 160 && shown.frames > 0 && shown.colours > 4);
  expect("AAC decodes", shown.audioBytes > 0, `${shown.audioBytes} bytes`);
  expect("negative control: a made-up codec is refused", shown.types.bogus === "");
  await closeGuest();
} else expect("the local video page opened", false, JSON.stringify(playerState));

await control("flushCookies", { partition: PARTITION });
server.close();
await driver.finish();

// Read once Chromium has let go of the file. On a kept-alive instance, from a copy.
const profile = path.join(driver.data, `Partitions/browser-profile-${PROFILE}`);
const cookies = [path.join(profile, "Network/Cookies"), path.join(profile, "Cookies")].find((file) => fs.existsSync(file));
if (cookies) {
  const copy = path.join(out, "cookies-copy");
  fs.copyFileSync(cookies, copy);
  const database = new DatabaseSync(copy, { readOnly: true });
  const row = database.prepare("SELECT value, hex(substr(encrypted_value,1,3)) AS prefix, length(encrypted_value) AS size FROM cookies WHERE name='brigadier_check'").get();
  database.close();
  const scheme = row ? Buffer.from(row.prefix, "hex").toString() : null;
  report.cookie = { scheme, size: row?.size ?? 0, plainColumnEmpty: row?.value === "", plainTextInFile: fs.readFileSync(copy).includes("made-up-cookie-value") };
  fs.rmSync(copy);
  expect("the made-up cookie is on disk encrypted", Boolean(scheme?.startsWith("v1")) && report.cookie.plainColumnEmpty && !report.cookie.plainTextInFile, JSON.stringify(report.cookie));
} else expect("the profile's cookie file exists", false, profile);

if (process.platform === "darwin" && process.env.BRIGADIER_REAL_KEYCHAIN === "1") {
  // A throwaway default Keychain the workflow made: the real items Chromium wrote, by service name.
  const dump = execFileSync("security", ["dump-keychain", process.env.BRIGADIER_SCRATCH_KEYCHAIN], { encoding: "utf8" });
  const items = [...dump.matchAll(/"svce"<blob>="([^"]*)"/g)].map((match) => match[1]);
  report.keychain = items;
  expect("the Keychain item is Brigadier's", items.includes("Brigadier Safe Storage"), JSON.stringify(items));
  expect("Chromium Safe Storage is not touched", !items.includes("Chromium Safe Storage"));
}

const table = (rows) => rows.map((row) => `| ${row.join(" | ")} |`).join("\n");
const summary = [
  `## Real pages: ${process.platform} ${process.arch}, Electron ${report.info.electron}, Chromium ${report.info.chrome}`,
  "Real engine and network; the guest made by script, no real input.", "",
  table([["Page", "Result", "Title"], ["---", "---", "---"], ...report.pages.map((row) => [row.url, row.failed ?? `${row.text} chars, ${row.colours} colours`, row.title ?? ""])]), "",
  table([["Process", "OS says sandboxed", "OS reading", "Chromium says"], ["---", "---", "---", "---"], ...report.sandbox.map((row) => [`${row.type}${row.service ? ` ${row.service}` : ""}${row.guest ? " (web page)" : ""}`, row.osSays, row.detail, row.chromiumSays])]), "",
  `H.264/AAC: \`${JSON.stringify(report.video)}\``, `Cookie on disk: \`${JSON.stringify(report.cookie)}\``, ...(report.keychain ? [`Keychain items: \`${JSON.stringify(report.keychain)}\``] : []), "",
  failures.length ? `**Failed:** ${failures.join("; ")}` : "**All passed.**", "",
].join("\n");
fs.writeFileSync(path.join(out, "real-pages.md"), summary);
fs.writeFileSync(path.join(out, "real-pages.json"), JSON.stringify(report, null, 2));
if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
console.log(`\n${failures.length ? `FAILED: ${failures.length}` : "all passed"}; report in ${out}`);
process.exit(failures.length ? 1 : 0);
