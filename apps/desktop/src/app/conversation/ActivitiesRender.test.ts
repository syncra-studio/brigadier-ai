import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Row = { label: string; state: string; moving: boolean; elapsed: string; tip: string } | null;

test("Background lists what still runs, one line each with its state and Stop on hover; a web preview opens in the browser, another preview opens its output in a tab, and what the user stopped leaves the card", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?drive=1", "activities-result", 60000)) as {
    empty: boolean; title: string; order: string[]; rows: Record<string, Row>; heights: number[]; workerLine: string | null; sameRow: boolean;
    stops: string[]; stopVisibleAtRest: boolean; browsed: string | null; outputs: boolean[]; opened: string | null; openedApp: string; grew: boolean; calls: string[]; left: string[];
  };
  assert.equal(seen.empty, true, "nothing running, no section");
  assert.equal(seen.title, "Background");
  // Newest first.
  assert.deepEqual(seen.order, ["preview-2", "preview-1"]);
  assert.deepEqual(seen.rows["preview-1"], { label: "Demo pages", state: "running", moving: false, elapsed: "12m", tip: "Showing: pnpm dev --port 5199" });
  // A build spins; the page that is up shows its live dot.
  assert.deepEqual([seen.rows["preview-2"]?.label, seen.rows["preview-2"]?.state, seen.rows["preview-2"]?.moving], ["Dev app", "building", true]);
  assert.match(seen.rows["preview-2"]?.tip ?? "", /^Building: pnpm tauri build[^]*\nFrom Demo of the fixes$/);
  assert.deepEqual(seen.heights, [28], "one line each");
  // The worker's line names it; what it runs shows in the Workers panel, not the card.
  assert.doesNotMatch(seen.workerLine ?? "", /cargo clippy --workspace/);
  assert.equal(seen.sameRow, true, "the build and the app are the same row");
  assert.deepEqual(seen.stops, ["Stop", "Stop"]);
  assert.equal(seen.stopVisibleAtRest, false, "a running row's Stop shows on hover only");
  // The web preview opens in Brigadier's browser; its output is its row's Show output.
  assert.equal(seen.browsed, "http://localhost:5199/");
  assert.deepEqual(seen.outputs, [true, false]);
  assert.equal(seen.opened, "panel:output:preview:preview-1 Demo pages");
  // An app preview opens what it prints.
  assert.equal(seen.openedApp, "panel:output:preview:preview-2");
  assert.equal(seen.grew, false, "nothing opens in the card");
  assert.deepEqual(seen.calls, ["stopPreview preview-1"]);
  assert.deepEqual(seen.left, ["preview-2"], "what the user stopped left the card");
});

test("a Background row leaves as soon as its process ends, failed, finished or stopped: no red row, no Restart and no ×", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?ended=1", "activities-result", 60000)) as {
    running: string[]; ended: string[]; section: boolean; restartOrDismiss: number;
  };
  assert.deepEqual(seen.running, ["preview-14", "preview-13", "preview-12", "preview-11"]);
  // A crash, a non-zero exit, a clean exit and a Stop all take their row off at once.
  assert.deepEqual(seen.ended, []);
  assert.equal(seen.section, false, "nothing runs, no section");
  assert.equal(seen.restartOrDismiss, 0);
});

type Look = { icon: string | null; moving: boolean; green: boolean; shimmer: boolean; sweeping: boolean };

test("Background rows: a spinner while a process is on its way, a green live dot once it is up, a pause sign while held; the word shimmers while it works, and nothing moves with less motion", { timeout: 120000 }, async (t) => {
  const looks = JSON.parse(await renderFixtureLive(t, "activities.html?states=1", "activities-result", 60000)) as Record<string, Look>;
  const busy: Look = { icon: "spinner", moving: true, green: false, shimmer: true, sweeping: true };
  const expected: Record<string, Look> = {
    building: busy,
    starting: busy,
    "smoke testing": busy,
    checking: busy,
    running: { icon: "live", moving: true, green: true, shimmer: true, sweeping: true },
    paused: { icon: "paused", moving: false, green: false, shimmer: false, sweeping: false },
  };
  assert.deepEqual(looks, expected);
  const still = JSON.parse(await renderFixtureLive(t, "activities.html?states=1", "activities-result", 60000, { reducedMotion: true })) as Record<string, Look>;
  for (const [state, look] of Object.entries(still)) {
    assert.deepEqual([look.moving, look.sweeping], [false, false], `${state} is still with less motion`);
    assert.equal(look.icon, expected[state]?.icon, `${state} keeps its icon`);
  }
});

test("Background shows CLI commands with Stop and output; native agents appear in Workers", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?tasks=1", "activities-result", 60000)) as {
    running: string[]; nativeWorkers: string[]; rows: Record<string, Row>; stops: string[]; buttons: string[]; opened: string; output: boolean; calls: string[];
    done: Record<string, string>; icons: string[]; agentLeft: boolean; agentLeftWith: string[]; shellLeft: boolean; stoppedOrFailed: string[]; section: boolean;
  };
  // Newest first, each named as the CLI names it.
  assert.deepEqual(seen.running, ["task:bdza4j3ng"]);
  assert.deepEqual(seen.nativeWorkers, ["Reply with hi working"]);
  assert.deepEqual(seen.rows["task:bdza4j3ng"], { label: "Sleep 20 seconds then echo marker", state: "running", moving: false, elapsed: "8s", tip: "sleeping" });
  assert.deepEqual(seen.stops, ["Stop"]);
  assert.deepEqual(seen.buttons, ["BUTTON"], "the command opens its output");
  assert.equal(seen.opened, "panel:output:task:bdza4j3ng");
  assert.equal(seen.output, true, "its output tab reads the CLI's output file");
  assert.deepEqual(seen.calls, ["stopBackgroundTask bdza4j3ng"]);
  assert.deepEqual(seen.done, { "task:bdza4j3ng": "done" });
  assert.deepEqual(seen.icons, ["done"]);
  assert.equal(seen.agentLeft, true, "native agents are absent from Background");
  assert.deepEqual(seen.agentLeftWith, ["task:bdza4j3ng"]);
  assert.equal(seen.shellLeft, true, "a command leaves 5 min after it exited");
  assert.deepEqual(seen.stoppedOrFailed, [], "stopped and failed tasks leave at once");
  assert.equal(seen.section, false);
});
