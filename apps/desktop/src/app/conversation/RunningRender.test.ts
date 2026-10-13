import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("Running renders all states and controls pause, resume, stop, clear and the session browser", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "running.html?drive=1", "running-result", 30000)) as {
    chip: string; linkClosed: boolean;
    initial: { name: string; text: string; actions: string[] }[];
    paused: string[]; resumed: string[]; windows: string[]; error: string; browser: string;
    cleared: string[]; stopped: string[]; allStopped: boolean; empty: string; calls: string[];
  };
  assert.equal(seen.chip, "Brigadier dev app· Starting");
  assert.equal(seen.initial.length, 5);
  assert.match(seen.initial[0]!.text, /Starting/);
  assert.match(seen.initial[0]!.text, /Building \[======> \] 521\/731: brigadier-core/);
  assert.doesNotMatch(seen.initial[0]!.text, /Running|localhost/);
  assert.ok(!seen.initial[0]!.text.includes("\u001b"));
  assert.deepEqual(seen.initial[0]!.actions, ["Pause", "Stop", "Show log"]);
  assert.match(seen.initial[1]!.text, /Running/);
  assert.match(seen.initial[2]!.text, /Paused/);
  assert.match(seen.initial[3]!.text, /Exited · code 1/);
  assert.match(seen.initial[4]!.text, /Stopped · stopped by the user/);
  assert.doesNotMatch(seen.initial[4]!.text, /localhost:1420/, "a desktop app gets no URL");
  assert.ok(seen.paused.includes("Resume"));
  assert.ok(seen.resumed.includes("Pause"));
  assert.ok(!seen.windows.includes("Pause") && !seen.windows.includes("Resume") && seen.windows.includes("Stop"));
  assert.match(seen.error, /preview has ended/);
  assert.equal(seen.linkClosed, true);
  assert.equal(seen.browser, "http://localhost:5173/");
  assert.deepEqual(seen.cleared, ["preview-5", "preview-4", "preview-3"]);
  assert.ok(!seen.stopped.includes("Stop") && !seen.stopped.includes("Pause"));
  assert.equal(seen.allStopped, true);
  assert.match(seen.empty, /No previews/);
  assert.deepEqual(seen.calls, ["pausePreview", "resumePreview", "pausePreview", "clearPreviews", "stopPreview", "stopPreview", "clearPreviews"]);
});
