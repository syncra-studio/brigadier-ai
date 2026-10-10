import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("Running renders all states and controls pause, resume, stop, clear and the session browser", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "running.html?drive=1", "running-result", 10000)) as {
    initial: { name: string; text: string; actions: string[] }[];
    paused: string[]; resumed: string[]; windows: string[]; error: string; browser: string;
    cleared: string[]; stopped: string[]; allStopped: boolean; empty: string; calls: string[];
  };
  assert.equal(seen.initial.length, 4);
  assert.match(seen.initial[0]!.text, /Running/);
  assert.match(seen.initial[1]!.text, /Paused/);
  assert.match(seen.initial[2]!.text, /Exited · code 1/);
  assert.match(seen.initial[3]!.text, /Stopped · stopped by the user/);
  assert.ok(seen.paused.includes("Resume"));
  assert.ok(seen.resumed.includes("Pause"));
  assert.ok(!seen.windows.includes("Pause") && !seen.windows.includes("Resume") && seen.windows.includes("Stop"));
  assert.match(seen.error, /preview has ended/);
  assert.equal(seen.browser, "http://localhost:5173/");
  assert.deepEqual(seen.cleared, ["preview-4", "preview-3"]);
  assert.ok(!seen.stopped.includes("Stop") && !seen.stopped.includes("Pause"));
  assert.equal(seen.allStopped, true);
  assert.match(seen.empty, /Previews started by this session appear here/);
  assert.deepEqual(seen.calls, ["pausePreview", "resumePreview", "pausePreview", "clearPreviews", "stopPreview", "stopPreview", "clearPreviews"]);
});
