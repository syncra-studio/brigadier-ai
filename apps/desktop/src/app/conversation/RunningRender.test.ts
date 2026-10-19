import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("Background lists the previews still there; a web preview's Show output opens its output tab, which pauses, resumes, stops and opens it", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "running.html?drive=1", "running-result", 30000)) as {
    section: string; order: string[]; chip: string; pausedIcon: boolean; opened: string; tabText: string; initial: string[];
    paused: string[]; resumed: string[]; windows: string[]; error: string; browser: string; stopped: boolean; ended: string;
    focusAfterStop: string; calls: string[]; endedLeft: boolean; reactivated: string; reactivatedTexts: string[];
    lastStopFocus: string; lastStopRemoved: boolean;
  };
  assert.match(seen.section, /^Background/);
  // Only what is still there, newest first: the two that ended are not in the card.
  assert.deepEqual(seen.order, ["preview-5", "preview-4", "preview-3"]);
  assert.match(seen.chip, /^Brigadier dev app1m/);
  assert.equal(seen.pausedIcon, true);
  assert.equal(seen.opened, "panel:output:preview:preview-4");
  assert.match(seen.tabText, /Web app.*Showing/);
  assert.match(seen.tabText, /Waiting for file changes/);
  assert.deepEqual(seen.initial, ["Open", "Pause", "Stop"]);
  assert.ok(seen.paused.includes("Resume"));
  assert.ok(seen.resumed.includes("Pause"));
  assert.ok(!seen.windows.includes("Pause") && !seen.windows.includes("Resume") && seen.windows.includes("Stop"));
  assert.match(seen.error, /preview has ended/);
  assert.equal(seen.browser, "http://localhost:5173/");
  assert.equal(seen.stopped, true, "a stopped preview leaves the card");
  assert.match(seen.ended, /It has ended/);
  assert.equal(seen.focusAfterStop, "summary-card");
  assert.deepEqual(seen.calls, ["pausePreview", "resumePreview", "pausePreview", "stopPreview", "stopPreview"]);
  assert.equal(seen.endedLeft, true);
  assert.match(seen.reactivated, /1m/, "elapsed time refreshes immediately when activity restarts");
  assert.ok(seen.reactivatedTexts.some((text) => /1m/.test(text)), JSON.stringify(seen.reactivatedTexts));
  assert.ok(!seen.reactivatedTexts.some((text) => /(?<!\d|m )0s/.test(text)), `the restarted row never shows 0s, even for one render: ${JSON.stringify(seen.reactivatedTexts)}`);
  assert.equal(seen.lastStopFocus, "summary-card");
  assert.equal(seen.lastStopRemoved, true);
});
