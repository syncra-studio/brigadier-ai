#!/usr/bin/env node
// Drops and notifications in the real window on the real daemon. The drag comes through Chromium's input
// layer: trusted events carrying real files, with no OS pointer and no real Shift key, so this proves the app's
// path from a drop to its attachment, not a person's drag from Finder. HOME and data are scratch.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { call } from "./bc.mjs";
import { capture } from "./instruments.mjs";
import { host, openShell, selectSession, sleep } from "./lib.mjs";
import { checkDir, eventsPath, evidenceDir } from "./paths.mjs";

const folder = path.join(evidenceDir, "drops");
const name = "app";
const results = [];
const check = (what, passed, detail = "") => {
  results.push({ what, passed, detail });
  console.log(`${passed ? "ok  " : "FAIL"} ${what}${detail ? `: ${detail}` : ""}`);
};
const page = (js) => host(name, js);
const invoke = (command, args = {}) => page(`(async()=>{const n=await import("/src/ipc/native.ts");try{const v=await n.invoke(${JSON.stringify(command)},${JSON.stringify(args)});return{value:v instanceof Uint8Array?{bytes:new TextDecoder().decode(v)}:v}}catch(error){return{error:String(error)}}})()`);
const ask = (request) => page(`(async()=>{const c=await import("/src/ipc/client.ts");return c.request(${JSON.stringify(request)})})()`);
const eventsFrom = (at) => fs.readFileSync(eventsPath, "utf8").trim().split("\n").map((line) => JSON.parse(line)).filter((event) => event.at >= at);
const drag = async (type, x, y, files, modifiers = 0) => call({ op: "drag", window: name, params: { type, x, y, modifiers, data: { items: [], files, dragOperationsMask: 1 } } });
const dropAt = async (x, y, files, settle = 1000) => {
  await drag("dragEnter", x, y, files);
  await drag("dragOver", x, y, files);
  await sleep(300);
  await drag("drop", x, y, files);
  await sleep(settle);
};

const started = new Date().toISOString();
const files = path.join(checkDir, "drop-files");
fs.rmSync(files, { recursive: true, force: true });
fs.mkdirSync(files, { recursive: true });
const notes = path.join(files, "notes for drop.txt");
fs.writeFileSync(notes, "dropped bytes\n");
const dropFolder = path.join(files, "dropped-project");
fs.mkdirSync(dropFolder);
execFileSync("git", ["init", "-q", dropFolder]);

const { windowId } = await openShell(name, { real: true });
for (let tries = 0; !(await page(`document.documentElement.dataset.platform === "macos" && !document.body.innerText.includes("Reconnecting")`)); tries++) {
  if (tries > 120) throw new Error("the app never connected");
  await sleep(250);
}
const { catalog } = await ask({ method: "getCatalog" });
await ask({ method: "updateSettings", settings: { ...catalog.settings, onboarded: true, introSeen: true, notifyFinished: "always" } });
await page(`location.reload(), true`).catch(() => {});
await sleep(1500);
for (let tries = 0; !(await page(`!document.getElementById("splash") && !!document.querySelector("[data-slot]")`)); tries++) {
  if (tries > 120) throw new Error("the app did not start again");
  await sleep(250);
}

const repo = path.join(checkDir, "drop-repo");
fs.mkdirSync(repo, { recursive: true });
if (!fs.existsSync(path.join(repo, ".git"))) {
  execFileSync("git", ["init", "-q", repo]);
  fs.writeFileSync(path.join(repo, "README.md"), "A throwaway project.\n");
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.name=Dev", "-c", "user.email=dev@example.invalid", "commit", "-qm", "start"]);
}
const before = await ask({ method: "getCatalog" });
const project = before.catalog.projects.find((each) => each.repos?.some((one) => one.path === repo)) ?? (await ask({ method: "addProject", path: repo, name: "Drop project", init: false })).project;
await ask({ method: "setFolderTrust", id: project.id, path: null, trusted: true });
const made = await ask({ method: "createConversation", kind: "session", projectId: project.id, title: "Drops", setup: null });
const session = (made.conversation ?? made.view?.conversation ?? made).id;
await selectSession(name, session);
await sleep(1000);

// Nothing is readable before a drop, and a page can't fake one.
const forged = await page(`(async()=>{const t=new DataTransfer();t.items.add(new File(["x"],"x.txt"));window.dispatchEvent(new DragEvent("drop",{dataTransfer:t,bubbles:true,cancelable:true}));await new Promise(r=>setTimeout(r,200));return true})()`);
const afterForged = await invoke("last_drop");
check("a drop forged by the page records nothing", forged === true && afterForged.value === null, JSON.stringify(afterForged));
const secret = await invoke("read_dropped", { path: notes });
check("before any drop, a file can't be read", /Only a dropped file/.test(secret.error ?? ""), secret.error);

const zone = await page(`(()=>{const e=document.querySelector('[data-slot="composer"], form, textarea, [contenteditable="true"]');const r=e.getBoundingClientRect();return{x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}})()`);
await drag("dragEnter", zone.x, zone.y, [notes]);
await drag("dragOver", zone.x, zone.y, [notes]);
await sleep(500);
await capture(windowId, "file-over-thread", folder);
const shift = await invoke("drag_shift");
check("drag_shift answers the OS's state with no key held", shift.value === false, JSON.stringify(shift));
await drag("drop", zone.x, zone.y, [notes]);
await sleep(1500);
const last = await invoke("last_drop");
check("last_drop has the real path of the file", last.value?.entries?.[0]?.path === notes && last.value.entries[0].folder === false && last.value.shift === false, JSON.stringify(last));
const read = await invoke("read_dropped", { path: notes });
check("read_dropped gives the file's bytes", read.value?.bytes === "dropped bytes\n", JSON.stringify(read));
const other = await invoke("read_dropped", { path: path.join(repo, "README.md") });
check("read_dropped refuses a path outside the drop", /Only a dropped file/.test(other.error ?? ""), other.error);
const tiles = await page(`(document.body.innerText.match(/notes for drop/g) ?? []).length`);
check("the file is attached in the composer, once", tiles === 1, `${tiles} tile(s)`);
await capture(windowId, "file-attached", folder);

// Shift in the event only: Chromium's input layer can't press the real key.
await drag("dragEnter", zone.x, zone.y, [notes], 8);
await drag("dragOver", zone.x, zone.y, [notes], 8);
await sleep(300);
await drag("drop", zone.x, zone.y, [notes], 8);
await sleep(800);
const typed = await page(`(document.querySelector('[contenteditable="true"], textarea')?.innerText ?? document.querySelector("textarea")?.value ?? "")`);
const shifted = await invoke("last_drop");
check("a Shift drop types the real path, once, and attaches nothing more", typed.split(notes).length === 2 && shifted.value?.shift === true && (await page(`(document.body.innerText.match(/TXT text/g) ?? []).length`)) === 1, JSON.stringify({ typed, shift: shifted.value?.shift }));
await capture(windowId, "shift-typed-path", folder);

// A folder dropped outside the thread becomes a project.
const edge = await page(`(()=>{const e=document.querySelector('[data-slot="sidebar"]')??document.body;const r=e.getBoundingClientRect();return{x:Math.round(r.x+Math.min(r.width,200)/2),y:Math.round(r.y+r.height/2)}})()`);
await dropAt(edge.x, edge.y, [dropFolder], 2500);
const folderDrop = await invoke("last_drop");
check("last_drop marks a folder as one", folderDrop.value?.entries?.[0]?.path === dropFolder && folderDrop.value.entries[0].folder === true, JSON.stringify(folderDrop));
const added = await ask({ method: "getCatalog" });
const projectMade = added.catalog.projects.some((each) => each.repos?.some((one) => one.path === fs.realpathSync(dropFolder) || one.path === dropFolder));
check("the folder became a project in the daemon", projectMade);
await capture(windowId, "folder-project", folder);

// Notifications, as the daemon asks for them: a finished request, with this setting on "always".
const permission = await invoke("notification_permission");
check("notification_permission answers from the OS", ["notAsked", "off", "allowed"].includes(permission.value), JSON.stringify(permission));
const sendAt = new Date().toISOString();
const chat = await ask({ method: "createConversation", kind: "chat", projectId: null, title: "Notify", setup: null });
const chatId = (chat.conversation ?? chat).id;
const sent = await page(`(async()=>{const c=await import("/src/ipc/client.ts");return c.request({method:"sendMessage",conversationId:${JSON.stringify(chatId)},text:"hello",attachments:[],mentions:[],steer:false}).then(()=>"sent",String)})()`);
console.log(`send: ${sent}`);
let notified = [];
for (let tries = 0; tries < 120 && !notified.length; tries++) {
  await sleep(500);
  notified = eventsFrom(sendAt).filter((event) => event.kind.startsWith("notification-") || event.kind === "notify");
}
console.log(`notification events: ${JSON.stringify(eventsFrom(sendAt).filter((event) => /notif/.test(event.kind)))}`);
await capture(windowId, "after-send", folder);

const during = eventsFrom(started);
check("no renderer exit or focus change", !during.some((event) => ["render-process-gone", "browser-window-focus", "did-become-active"].includes(event.kind)));
const errors = [...new Set(during.filter((event) => event.kind === "console-error").map((event) => event.message.slice(0, 160)))];
console.log(`console errors, each once:\n  ${errors.join("\n  ")}`);
fs.writeFileSync(path.join(folder, "results.json"), JSON.stringify(results, null, 1));
const failed = results.filter((each) => !each.passed);
console.log(failed.length ? `${failed.length} FAILED` : `all ${results.length} passed (scripted drag, OS pixels; no real pointer)`);
process.exit(failed.length ? 1 : 0);
