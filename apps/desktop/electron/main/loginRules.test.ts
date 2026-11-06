import assert from "node:assert/strict";
import test from "node:test";

import { autostartDir, desktopEntry, execArg, loginItemTook, macLoginItem, privacyPane } from "./loginRules.ts";

test("SMAppService's states read as the Tauri build's", () => {
  assert.equal(macLoginItem("not-registered"), "off");
  assert.equal(macLoginItem("not-found"), "off");
  assert.equal(macLoginItem("enabled"), "on");
  assert.equal(macLoginItem("requires-approval"), "needsApproval");
  assert.equal(macLoginItem("anything else"), "unsupported");
});

test("a login item that didn't take shows as a failure, and waiting for approval isn't one", () => {
  assert.equal(loginItemTook(true, "on"), true);
  assert.equal(loginItemTook(true, "needsApproval"), true);
  assert.equal(loginItemTook(true, "off"), false);
  assert.equal(loginItemTook(false, "off"), true);
  assert.equal(loginItemTook(false, "on"), false);
});

test("each permission opens its own privacy list, and nothing else opens", () => {
  assert.match(privacyPane("accessibility"), /\?Privacy_Accessibility$/);
  assert.match(privacyPane("screenRecording"), /\?Privacy_ScreenCapture$/);
  for (const other of ["constructor", "camera", null, undefined]) assert.throws(() => privacyPane(other), /unknown permission/);
});

test("exec paths are quoted for desktop entries, as in login_item.rs's test", () => {
  assert.equal(execArg("/opt/Brigadier/brigadier-app"), '"/opt/Brigadier/brigadier-app"');
  assert.equal(execArg("/home/a b/Brigadier.AppImage"), '"/home/a b/Brigadier.AppImage"');
  assert.equal(execArg('/tmp/$x`y"%'), '"/tmp/\\\\$x\\\\`y\\\\"%%"');
  assert.equal(execArg("/tmp/a\\b"), '"/tmp/a\\\\\\\\b"');
  const entry = desktopEntry("Brigadier", "/opt/b");
  assert.ok(entry.startsWith("[Desktop Entry]\nType=Application\nName=Brigadier\n"));
  assert.ok(entry.includes('\nExec="/opt/b"\n'));
});

test("the autostart folder follows XDG, except a relative one", () => {
  assert.equal(autostartDir({ XDG_CONFIG_HOME: "/x", HOME: "/h" }), "/x/autostart");
  assert.equal(autostartDir({ XDG_CONFIG_HOME: "rel", HOME: "/h" }), "/h/.config/autostart");
  assert.equal(autostartDir({}), null);
});
