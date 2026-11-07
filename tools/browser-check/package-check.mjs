#!/usr/bin/env node
// The packaged app in the kept-alive instance (`BRIGADIER_CHECK_PACKAGE=<Brigadier.app> bc.mjs up`): addon, daemon,
// preload and page all from inside the bundle, under the page's real origin and policy. A <webview> under that
// policy, the updater against a local feed, memory by process. Scripted calls, OS pixels; no real pointer.
//   package-check.mjs [feed zip]
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

import { FIXTURES, control, host, openShell, sleep } from "./lib.mjs";
import { capture } from "./instruments.mjs";
import { eventsPath, evidenceDir } from "./paths.mjs";

const name = "app";
const folder = path.join(evidenceDir, "package");
const started = new Date().toISOString();
const check = (what, passed, detail = "") => console.log(`${passed ? "ok  " : "FAIL"} ${what}${detail ? `: ${detail}` : ""}`);
const invoke = (command, args = {}) => host(name, `window.brigadierNative.invoke(${JSON.stringify(command)},${JSON.stringify(args)}).then((a)=>JSON.stringify(a))`).then((text) => JSON.parse(text));
const { windowId } = await openShell(name, { real: true });
await sleep(4000);
check("the page is on its origin", (await host(name, "location.origin")) === "app://brigadier");
const info = await invoke("app_info");
check("the daemon answers through the bundle's addon", Boolean(info.value?.platform), JSON.stringify(info.value));

// A guest under the page's policy.
await host(name, `(()=>{const w=document.createElement("webview");w.id="csp-guest";w.setAttribute("partition","persist:browser-profile-default");w.src=${JSON.stringify(`${FIXTURES}/page/a`)};w.style.cssText="position:fixed;left:300px;top:100px;width:600px;height:400px;z-index:99999";document.body.appendChild(w);return true})()`);
await sleep(3000);
const guests = await control("guests");
check("a <webview> loads under the page's policy, in its own renderer", guests.some((guest) => guest.url.includes("/page/a") && guest.pid), JSON.stringify(guests.map((guest) => guest.url)));
await capture(windowId, "guest-under-policy", folder);
await host(name, `document.getElementById("csp-guest").remove(),true`);

const zip = process.argv[2];
if (zip) {
  const bytes = fs.readFileSync(zip);
  const sha512 = createHash("sha512").update(bytes).digest("base64");
  const file = path.basename(zip);
  const feed = `version: 9.9.9\nfiles:\n  - url: ${file}\n    sha512: ${sha512}\n    size: ${bytes.length}\npath: ${file}\nsha512: ${sha512}\nreleaseDate: '2026-11-06T00:00:00.000Z'\n`;
  const asked = [];
  const server = http.createServer((request, response) => {
    asked.push(request.url);
    if (request.url.startsWith("/latest-mac.yml")) response.end(feed);
    else if (request.url.startsWith(`/${file}`)) response.end(bytes);
    else { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve) => server.listen(1499, "127.0.0.1", resolve));
  const none = await invoke("app_update");
  check("before a check there's nothing offered", none.value === null || none.value === undefined);
  const offered = await invoke("check_for_updates");
  check("a newer build in the feed is offered, nothing downloaded", offered.value?.state === "available" && offered.value?.version === "9.9.9" && !asked.some((url) => url.startsWith(`/${file}`)), JSON.stringify(offered));
  await invoke("take_app_update");
  let state = null;
  for (let tries = 0; tries < 240; tries++) {
    await sleep(500);
    state = (await invoke("app_update")).value;
    if (state?.state !== "updating") break;
  }
  console.log(`after the click: ${JSON.stringify(state)}; the feed served ${asked.filter((url) => url.startsWith(`/${file}`)).length} download request(s). "ready" means downloaded and accepted; putting it in place on quit is Squirrel's and needs a signed build.`);
  server.close();
}

// Settled, and on the same footing as the Tauri reading: its page, idle, with no guest.
await sleep(15000);
const metrics = await control("metrics");
const total = metrics.reduce((sum, each) => sum + (each.kb ?? 0), 0);
console.log(`Electron processes, idle: ${metrics.map((each) => `${each.type} ${Math.round(each.kb / 1024)}`).join(", ")}; total ${Math.round(total / 1024)} MB; load ${os.loadavg().map((value) => value.toFixed(1)).join(" ")}`);
const events = fs.readFileSync(eventsPath, "utf8").trim().split("\n").map((line) => JSON.parse(line)).filter((event) => event.at >= started);
check("no renderer exit, no focus taken", !events.some((event) => ["render-process-gone", "browser-window-focus", "did-become-active"].includes(event.kind)));
console.log(`console errors: ${JSON.stringify([...new Set(events.filter((event) => event.kind === "console-error").map((event) => event.message.slice(0, 160)))])}`);
process.exit(0);
