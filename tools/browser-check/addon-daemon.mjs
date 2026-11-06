#!/usr/bin/env node
// The addon driving a real brigadierd with no Electron and no window: connect, ask, get the daemon's
// error back as data, hear an event of the shell's own, drain and stop. Real daemon, real socket; HOME and
// the data folder are scratch, so it reads no sign-in and raises no Keychain dialog.
import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";

import { addonPath, checkDir, sidecarDir } from "./paths.mjs";

const scratch = path.join(checkDir, `addon-daemon-${process.pid}`);
const data = path.join(scratch, "data");
fs.mkdirSync(path.join(scratch, "home"), { recursive: true });
process.env.HOME = path.join(scratch, "home");

const native = createRequire(import.meta.url)(addonPath);
const events = [];
const shell = [];
const waitFor = async (what, ready, ms = 20000) => {
  const start = Date.now();
  while (!ready()) {
    if (Date.now() - start > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
};

const started = await native.start(
  { identifier: "ai.brigadier.dev", dataDir: data, sidecarDir, daemonEnv: [] },
  (json) => shell.push(JSON.parse(json)),
);
assert.equal(started.dataDir, data);
assert.equal(started.firstLaunch, true);
native.attachUi((json) => events.push(JSON.parse(json)));

const coldAt = Date.now();
await waitFor("the daemon to connect", () => events.some((event) => event.type === "connected"));
const connected = events.find((event) => event.type === "connected");
console.log(`connected in ${Date.now() - coldAt} ms to daemon pid ${connected.daemon.pid}, data ${connected.daemon.dataDir}`);
assert.equal(connected.daemon.dataDir, data);
// A failed assertion must not leave the scratch daemon behind.
process.on("exit", () => {
  try {
    process.kill(connected.daemon.pid);
  } catch { /* already gone */ }
});

const catalog = await native.request(JSON.stringify({ method: "getCatalog" }));
assert.equal(catalog.error, undefined);
assert.equal(JSON.parse(catalog.response).method, "getCatalog");

const refused = await native.request(JSON.stringify({ method: "getConversation", id: "no-such-conversation", limit: 1 }));
const error = JSON.parse(refused.error ?? "{}");
assert.ok(error.code && error.message, `a daemon's error reaches JS as data: ${JSON.stringify(refused)}`);
console.log(`daemon's error as data: ${error.code}: ${error.message}`);

await assert.rejects(native.request("not json"));

native.emit(JSON.stringify({ type: "settingsMenu" }));
await waitFor("the shell's own event", () => events.some((event) => event.type === "settingsMenu"), 2000);

assert.equal(native.decideOpening("https://example.com", false).kind, "refuse");
assert.equal(native.decideOpening(path.join(scratch, "home"), false).kind, "open");

const pid = connected.daemon.pid;
assert.equal(await native.shutdownDaemon(10000), true);
await waitFor("the daemon to exit", () => {
  try {
    process.kill(pid, 0);
    return false;
  } catch {
    return true;
  }
}, 10000);
console.log("daemon drained, acknowledged and exited");
console.log(`system: ${native.systemName()}`);
fs.rmSync(scratch, { recursive: true, force: true });
process.exit(0);
