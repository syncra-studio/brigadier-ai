import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const IMPORT = 'import { ipcRenderer } from "electron";';

/** The real preload as a page can run it. */
function preload(): string {
  const source = readFileSync(new URL("../../../electron/guestPasswords.ts", import.meta.url), "utf8");
  assert.ok(source.includes(IMPORT), "the preload's one import changed: the fixture's stand-in must follow");
  return stripTypeScriptTypes(source.replace(IMPORT, "const ipcRenderer = window.guestIpc;"));
}

const attempt = (username: string, password: string) => ["guest-login-attempt".slice("guest-login-".length), { username, password }];
const report = (hasPassword: boolean, fillable: boolean) => ({ hasPassword, fillable });

// The guests' real preload in Chromium, with its layout and trusted input: what it takes for a sign-in, and what it fills.
test("sign-in detection and filling on real pages", { timeout: 90000 }, async (t) => {
  const code = Buffer.from(preload()).toString("base64");
  const seen = JSON.parse(await renderFixtureLive(t, `guest-passwords.html?code=${encodeURIComponent(code)}`, "guest-result", 80000)) as Record<string, unknown>;
  assert.equal(seen.failed, undefined);

  assert.deepEqual(seen.signIn, {
    reports: [report(true, true)],
    // A page's own script can't press for the user.
    scriptedPress: [],
    pressed: [attempt("fake-sam", "fake-Tulip-41")],
    enter: [attempt("fake-sam", "fake-Tulip-41")],
  });

  assert.deepEqual(seen.filling, {
    // Main named another site: the page moved on, and nothing lands.
    otherSite: ["", ""],
    // What the user typed stays, and the page hears the rest as typing.
    typedStays: ["typed-by-user", "fake-Tulip-41"],
    events: ["input", "change"],
    noPassword: ["fake-sam", ""],
  });

  // Signing up or changing a password isn't filled.
  assert.deepEqual(seen.twoPasswords, { report: report(true, false), filled: [] });
  assert.deepEqual(seen.newPassword, { report: report(true, false), filled: [] });

  // A box nobody can see isn't there: the page reads as a name-first step, and the password stays out of it.
  assert.deepEqual(seen.hiddenPassword, { report: report(false, true), filled: ["name"] });
  assert.deepEqual(seen.invisiblePassword, { report: report(false, true), filled: ["name"] });
  assert.deepEqual(seen.tinyPassword, { report: report(false, true), filled: ["name"] });
  assert.deepEqual(seen.hiddenBoxes, { caught: [] });

  // A lone box that isn't named for a login is no name step.
  assert.deepEqual(seen.searchBox, { report: report(false, false), filled: [] });

  // A box marked as the username wins over the one before the password.
  assert.deepEqual(seen.markedName, { report: report(true, true), filled: ["marked", "pw"] });

  assert.deepEqual(seen.nameFirst, {
    first: report(false, true),
    nameFilled: "fake-sam",
    caught: [["name", "fake-sam"]],
    second: report(true, true),
    passwordFilled: "fake-Tulip-41",
  });

  assert.deepEqual(seen.otherForm, {
    searchButton: [],
    link: [],
    formlessButton: [attempt("fake-sam", "fake-Tulip-41")],
  });

  assert.deepEqual(seen.lateForm, {
    atLoad: [],
    settled: [report(false, false)],
    drawn: report(true, true),
    gone: report(false, false),
  });
});
