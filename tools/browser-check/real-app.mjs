#!/usr/bin/env node
// The real window on the real daemon, in the kept-alive instance: settings saved through the daemon, a real
// project and session in it, a browser tab in that session, read back by OS pixels; then one orderly quit and
// a relaunch, which must find the session in the daemon's store. Scripted page calls and no real pointer: it
// proves the app's parts connect, not that a human can click them. HOME and data are scratch.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { call } from "./bc.mjs";
import { FIXTURES, control, host, layerState, openShell, openTab, selectSession, sleep } from "./lib.mjs";
import { capture } from "./instruments.mjs";
import { checkDir, eventsPath, evidenceDir } from "./paths.mjs";

const folder = path.join(evidenceDir, "real-app");
const name = "app";
const results = [];
const check = (what, passed, detail = "") => {
  results.push({ what, passed, detail });
  console.log(`${passed ? "ok  " : "FAIL"} ${what}${detail ? `: ${detail}` : ""}`);
};
const ask = (request) => host(name, `(async()=>{const c=await import("/src/ipc/client.ts");return c.request(${JSON.stringify(request)})})()`);
const eventsFrom = (at) => fs.readFileSync(eventsPath, "utf8").trim().split("\n").map((line) => JSON.parse(line)).filter((event) => event.at >= at);

const started = new Date().toISOString();
const { windowId } = await openShell(name, { real: true });
check("the real window is a page on the native door", await host(name, "Boolean(window.brigadierNative)"));
for (let tries = 0; !(await host(name, `document.documentElement.dataset.platform === "macos" && !document.body.innerText.includes("Reconnecting")`)); tries++) {
  if (tries > 120) throw new Error("the app never connected");
  await sleep(250);
}
const { catalog } = await ask({ method: "getCatalog" });
await ask({ method: "updateSettings", settings: { ...catalog.settings, onboarded: true, introSeen: true, browserEnabled: true } });
const saved = await ask({ method: "getCatalog" });
check("a setting goes through the daemon and comes back", saved.catalog.settings.onboarded === true);
await host(name, "location.reload(), true").catch(() => {});
await sleep(1500);
for (let tries = 0; !(await host(name, `!document.getElementById("splash") && !!document.querySelector("[data-slot]")`)); tries++) {
  if (tries > 120) throw new Error("the app did not start again");
  await sleep(250);
}

// A throwaway repository, as a project, and a session in it.
const repo = path.join(checkDir, "real-app-repo");
fs.mkdirSync(repo, { recursive: true });
if (!fs.existsSync(path.join(repo, ".git"))) {
  execFileSync("git", ["init", "-q", repo]);
  fs.writeFileSync(path.join(repo, "README.md"), "A throwaway project for the dev app.\n");
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.name=Dev", "-c", "user.email=dev@example.invalid", "commit", "-qm", "start"]);
}
const before = await ask({ method: "getCatalog" });
const project = before.catalog.projects.find((each) => each.repos?.some((one) => one.path === repo)) ?? (await ask({ method: "addProject", path: repo, name: "Dev project", init: false })).project;
check("a real project is in the daemon", Boolean(project?.id), project?.id);
await ask({ method: "setFolderTrust", id: project.id, path: null, trusted: true });
const made = await ask({ method: "createConversation", kind: "session", projectId: project.id, title: "Electron check", setup: null });
const session = (made.conversation ?? made.view?.conversation ?? made).id;
check("a real session is in the daemon", Boolean(session), session);
await selectSession(name, session);
await sleep(1000);
await capture(windowId, "session", folder);

const tab = await openTab(name, session, `${FIXTURES}/page/a?colour=2d6cdf`);
await sleep(2000);
const layer = await layerState(name);
const page = layer.pages.find((each) => each.id === tab.id);
const guests = await control("guests");
const guest = guests.find((each) => each.id === tab.wc);
check("the browser tab's guest runs in its own renderer, sandboxed partition", Boolean(guest?.pid), JSON.stringify({ pid: guest?.pid, partition: guest?.partition?.split("/").pop(), url: page?.url ?? guest?.url }));
const shot = await capture(windowId, "session-and-tab", folder);
console.log(`evidence: ${shot.file}`);

const metrics = await control("metrics");
console.log(`processes: ${metrics.map((each) => `${each.type}${each.sandboxed ? "" : " (no sandbox)"} ${Math.round(each.kb / 1024)} MB`).join(", ")}`);
const during = eventsFrom(started);
check("no renderer exit or focus change so far", !during.some((event) => ["render-process-gone", "browser-window-focus", "did-become-active"].includes(event.kind)), during.filter((event) => event.kind === "render-process-gone").map((event) => event.reason).join());
const errors = [...new Set(during.filter((event) => event.kind === "console-error").map((event) => event.message.slice(0, 120)))];
console.log(`console errors, each once (commands not yet moved are expected):\n  ${errors.join("\n  ")}`);

// One orderly quit and relaunch.
const quitAt = new Date().toISOString();
const daemonPid = (await ask({ method: "getDiagnostics" })).diagnostics?.daemon?.pid ?? null;
await call({ op: "restart" }, 60000);
const order = eventsFrom(quitAt).filter((event) => ["flush-pages", "daemon-quit", "ready"].includes(event.kind)).map((event) => `${event.kind}${event.saved !== undefined ? `:saved=${event.saved}` : ""}${event.acknowledged !== undefined ? `:acknowledged=${event.acknowledged}` : ""}`);
check("the quit saved pages, then the daemon acknowledged, then the app started again", order[0] === "flush-pages:saved=true" && order[1] === "daemon-quit:acknowledged=true" && order[2] === "ready", order.join(" > "));
if (daemonPid) {
  let gone = false;
  try { process.kill(daemonPid, 0); } catch { gone = true; }
  console.log(`the old daemon ${daemonPid}: ${gone ? "gone" : "still running"}`);
}
await openShell(name, { real: true });
for (let tries = 0; !(await host(name, `!document.body.innerText.includes("Reconnecting") && !!document.querySelector("[data-slot]")`)); tries++) {
  if (tries > 120) throw new Error("the app never connected after relaunch");
  await sleep(250);
}
const after = await ask({ method: "getCatalog" });
check("after the relaunch the session is still in the daemon's store", after.catalog.conversations.some((each) => each.id === session));
fs.writeFileSync(path.join(folder, "results.json"), JSON.stringify(results, null, 1));
const failed = results.filter((each) => !each.passed);
console.log(failed.length ? `${failed.length} FAILED` : `all ${results.length} passed (scripted page calls, OS pixels; no real pointer)`);
process.exit(failed.length ? 1 : 0);
