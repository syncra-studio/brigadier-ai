import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Look = {
  accessibility: { state: string; buttons: string[]; grantDisabled: boolean | null };
  screenRecording: { state: string; buttons: string[]; grantDisabled: boolean | null };
  note: string;
  setup: string;
  setupButtons: string[];
  menuBarDisabled: boolean | null;
};
type States = "unknown" | "cached" | "missingBoth" | "missingAccessibility" | "confirmCapture" | "ready" | "readyCached" | "restarting" | "failed" | "off";
type Seen = Record<States, Look> & { first: Look; checked: Look; sections: string[]; rows: string[]; listingCalls: string[]; calls: string[] };

// The real page in Chromium over a faked daemon.
test("computer use settings: enable, menu bar, permissions and setup, in every state", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "computer-use.html?drive=1", "computer-use-result", 15000)) as Seen;
  assert.deepEqual(seen.sections, ["Enable", "Menu bar", "Permissions", "Setup"]);
  assert.deepEqual(seen.rows, ["Let sessions use apps on this Mac", "Show in menu bar", "Accessibility", "Screen Recording", "Setup"]);
  // Permissions nothing has read yet (no helper has run for this data folder), with computer use
  // on: the page asks the helper once and shows what it holds, "Checking…" meanwhile.
  assert.deepEqual(seen.listingCalls, ["getComputerUse", "checkComputerUse"]);
  assert.equal(seen.first.accessibility.state, "Checking…");
  assert.equal(seen.first.screenRecording.state, "Checking…");
  assert.deepEqual(seen.first.accessibility.buttons, ["Open System Settings"]);
  assert.equal(seen.first.setup, "Checking…");
  assert.equal(seen.checked.accessibility.state, "Granted");
  assert.equal(seen.checked.screenRecording.state, "Granted");
  // Checked once: permissions unread again later show as unknown, and start nothing.
  assert.equal(seen.calls.filter((call) => call === "checkComputerUse").length, 1);

  const unknown = seen.unknown;
  assert.equal(unknown.accessibility.state, "Unknown");
  assert.equal(unknown.screenRecording.state, "Unknown");
  assert.deepEqual(unknown.accessibility.buttons, ["Grant…", "Open System Settings"]);
  assert.equal(unknown.setup, "Not set up yet. It starts the first time an agent needs it.");
  assert.deepEqual(unknown.setupButtons, ["Finish setup…"]);
  assert.equal(unknown.note, "");

  // What the helper last wrote is marked as such.
  assert.equal(seen.cached.accessibility.state, "Granted");
  assert.deepEqual(seen.cached.accessibility.buttons, ["Open System Settings"]);
  assert.equal(seen.cached.screenRecording.state, "Not granted");
  assert.equal(seen.cached.note, "As the helper last saw them.");
  assert.equal(seen.cached.setup, "Almost done: Screen Recording isn't allowed yet.");

  assert.equal(seen.missingBoth.note, "");
  assert.equal(seen.missingBoth.setup, "Almost done: Accessibility and Screen Recording aren't allowed yet.");
  assert.equal(seen.missingAccessibility.setup, "Almost done: Accessibility isn't allowed yet.");
  assert.deepEqual(seen.missingAccessibility.screenRecording.buttons, ["Open System Settings"]);

  assert.equal(seen.confirmCapture.setup, "Almost done: Brigadier still needs to check that it can see app windows.");
  assert.deepEqual(seen.confirmCapture.setupButtons, ["Finish setup…"]);

  assert.equal(seen.ready.setup, "Ready. Agents can use apps on this Mac.");
  assert.deepEqual(seen.ready.setupButtons, []);
  assert.equal(seen.readyCached.setup, "Set up. It starts when an agent needs it.");
  assert.equal(seen.restarting.setup, "The helper isn't responding. Brigadier is restarting it.");
  assert.deepEqual(seen.restarting.setupButtons, []);
  assert.equal(seen.failed.setup, "Computer use isn't working right now: the helper app is missing from this install");
  assert.deepEqual(seen.failed.setupButtons, ["Finish setup…"]);

  // Off: the menu-bar switch and Grant… wait for it, and setup has nothing to finish.
  assert.equal(seen.ready.menuBarDisabled, false);
  assert.equal(seen.off.menuBarDisabled, true);
  assert.equal(seen.off.screenRecording.grantDisabled, true);
  assert.equal(seen.off.setup, "Off. Agents can't use apps on this Mac.");
  assert.deepEqual(seen.off.setupButtons, []);

  assert.deepEqual(seen.calls.slice(2), [
    "grantComputerPermission screenRecording",
    "getComputerUse",
    "open_privacy_settings accessibility",
    "open_computer_setup",
  ]);
});
