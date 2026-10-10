import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Rendering = {
  first: { status: string; buttons: string[] };
  asked: { buttons: string[]; seeTheScreen: string };
  checked: boolean;
  ready: { status: string; buttons: string[] };
  calls: string[];
  focusable: boolean;
};

// The real page in Chromium over faked permissions: Allow… asks and adds the quiet controls,
// Open System Settings only opens the list, Start over asks afresh, and Check again reads at once.
test("computer use's page asks, opens System Settings, starts over and checks again", { timeout: 60000 }, async (t) => {
  const rendering = JSON.parse(await renderFixturePage(t, "computer-use.html?drive=1", "computer-use-result", 8000)) as Rendering;
  assert.equal(rendering.first.status, "Finish setup2 permissions needed");
  assert.deepEqual(rendering.first.buttons, ["Check again", "Allow…", "Allow…", "Where to find it"]);
  assert.deepEqual(rendering.asked.buttons, [
    "Check again",
    "Allow…",
    "Open System Settings",
    "Allow…",
    "Start over",
    "Where to find it",
  ]);
  assert.match(rendering.asked.seeTheScreen, /Not allowed/);
  assert.equal(rendering.checked, true);
  assert.equal(rendering.ready.status, "ReadyWorkers can see and use apps on this Mac.");
  assert.deepEqual(rendering.ready.buttons, ["Check again", "Open System Settings", "Open System Settings", "Where to find it"]);
  assert.deepEqual(rendering.calls, ["allow screenRecording", "open screenRecording", "allow screenRecording startOver"]);
  assert.ok(rendering.focusable);
});
