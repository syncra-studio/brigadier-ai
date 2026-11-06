import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const config = createRequire(import.meta.url)("./builder.config.cjs");

test("Windows installs per user into Programs and keeps the app's data at uninstall", () => {
  assert.equal(config.nsis.oneClick, true);
  assert.equal(config.nsis.perMachine, false);
  assert.equal(config.nsis.allowToChangeInstallationDirectory, false);
  assert.equal(config.nsis.deleteAppDataOnUninstall, false);
});

test("the Mac build is one architecture at a time, sidecars in Contents/MacOS, the id unchanged", () => {
  assert.equal(config.appId, "ai.brigadier.app");
  assert.deepEqual(config.mac.target, ["dmg", "zip"]);
  assert.equal(JSON.stringify(config).includes("universal"), false);
  assert.equal(config.mac.extraFiles[0].to, "MacOS/brigadierd");
});

test("the fuses shut what a packaged app must not open", () => {
  assert.equal(config.electronFuses.runAsNode, false);
  assert.equal(config.electronFuses.enableCookieEncryption, true);
  assert.equal(config.electronFuses.onlyLoadAppFromAsar, true);
});

test("the app's executable can't be mistaken for a sidecar on a case-insensitive disk", () => {
  assert.equal(config.win.executableName, "brigadier-app");
  assert.equal(config.linux.executableName, "brigadier-app");
  // macOS names the bundle after the executable, so afterPack.cjs renames it and places the CLI.
  assert.equal(config.executableName, undefined);
  assert.equal(config.afterPack, "electron/afterPack.cjs");
  assert.equal(config.mac.extraFiles.some((file: { to: string }) => file.to.toLowerCase() === "macos/brigadier"), false);
});
