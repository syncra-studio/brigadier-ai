import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Row = { label: string; state: string; moving: boolean; elapsed: string; tip: string } | null;

test("Background lists what still runs, one line each with its state and Stop on hover; a web preview opens in the browser, the rest open their output in a tab, and what the user stopped leaves the card", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?drive=1", "activities-result", 60000)) as {
    empty: boolean; title: string; order: string[]; rows: Record<string, Row>; heights: number[]; workerLine: string | null; sameRow: boolean;
    stops: string[]; stopVisibleAtRest: boolean; browsed: string | null; outputs: boolean[]; opened: string | null; openedApp: string; openedRun: string; grew: boolean; calls: string[]; left: string[];
  };
  assert.equal(seen.empty, true, "nothing running, no section");
  assert.equal(seen.title, "Background");
  // Newest first: the previews and the thread's long commands in one list.
  assert.deepEqual(seen.order, ["run-1", "run-2", "preview-2", "preview-1"]);
  assert.deepEqual(seen.rows["preview-1"], { label: "Demo pages", state: "running", moving: false, elapsed: "12m", tip: "Showing: pnpm dev --port 5199" });
  // A build and a check spin; the page that is up shows its live dot.
  assert.deepEqual([seen.rows["preview-2"]?.label, seen.rows["preview-2"]?.state, seen.rows["preview-2"]?.moving], ["Dev app", "building", true]);
  assert.match(seen.rows["preview-2"]?.tip ?? "", /^Building: pnpm tauri build[^]*\nFrom Demo of the fixes$/);
  assert.deepEqual(seen.rows["run-1"], { label: "Core tests", state: "checking", moving: true, elapsed: "2m", tip: "cargo test -p brigadier-core" });
  // A command the thread gave no name reads as its command, cut short.
  assert.equal(seen.rows["run-2"]?.label, "pnpm lint --max-warnings 0 --cache --ca…");
  assert.deepEqual(seen.heights, [28], "one line each");
  // The worker's line names it; what it runs shows in the Workers panel, not the card.
  assert.doesNotMatch(seen.workerLine ?? "", /cargo clippy --workspace/);
  assert.equal(seen.sameRow, true, "the build and the app are the same row");
  assert.deepEqual(seen.stops, ["Stop", "Stop", "Stop", "Stop"]);
  assert.equal(seen.stopVisibleAtRest, false, "a running row's Stop shows on hover only");
  // The web preview opens in Brigadier's browser; its output is its row's Show output.
  assert.equal(seen.browsed, "http://localhost:5199/");
  assert.deepEqual(seen.outputs, [true, false, false]);
  assert.equal(seen.opened, "panel:output:preview:preview-1 Demo pages");
  // An app preview and a command open what they print.
  assert.equal(seen.openedApp, "panel:output:preview:preview-2");
  assert.equal(seen.openedRun, "panel:output:run:run-1");
  assert.equal(seen.grew, false, "nothing opens in the card");
  assert.deepEqual(seen.calls, ["stopBackgroundRun run-1", "stopPreview preview-1"]);
  assert.deepEqual(seen.left, ["run-2", "preview-2"], "what the user stopped left the card");
});

test("a Background row leaves as soon as its process ends, failed, finished or stopped: no red row, no Restart and no ×", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?ended=1", "activities-result", 60000)) as {
    running: string[]; ended: string[]; section: boolean; restartOrDismiss: number;
  };
  assert.deepEqual(seen.running, ["preview-14", "preview-13", "preview-12", "preview-11", "run-13", "run-14", "run-15"]);
  // A crash, a non-zero exit, an interruption, a clean exit and a Stop all take their row off at once.
  assert.deepEqual(seen.ended, []);
  assert.equal(seen.section, false, "nothing runs, no section");
  assert.equal(seen.restartOrDismiss, 0);
});

test("Background shows the session's events while they run: the full checks, a review, CI with Open CI, a held merge with Cancel, a merge at work, a refused one with Merge anyway; each opens Progress and leaves once it ends", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?events=1", "activities-result", 60000)) as {
    order: string[]; rows: Record<string, Row>; buttons: Record<string, string[]>; buttonsVisibleAtRest: boolean; opened: string | null; calls: string[];
    ended: boolean; next: Record<string, string>; merged: string[]; restartOrDismiss: number;
    refused: Row; refusedButtons: string[]; refusedVisibleAtRest: boolean; anyway: string[];
  };
  // Newest first, each with its state and how far it got.
  assert.deepEqual(seen.order, ["merge", "ci", "review-1", "check-1"]);
  assert.deepEqual(seen.rows["check-1"], { label: "Full checks", state: "checking · 3 of 5", moving: true, elapsed: "5m", tip: "Now: pnpm test" });
  assert.deepEqual(seen.rows["review-1"], { label: "Codex review", state: "reviewing", moving: true, elapsed: "4m", tip: "The changes up to 4e1c49e" });
  assert.deepEqual(seen.rows["ci"], { label: "CI", state: "checking · 3 of 5 jobs", moving: true, elapsed: "3m", tip: "main · 4e1c49e9" });
  assert.deepEqual(seen.rows["merge"], { label: "Merge into main", state: "waiting · on 1 worker", moving: true, elapsed: "2m", tip: "Merging after 1 worker finishes" });
  assert.deepEqual(seen.buttons, { merge: ["Cancel"], ci: ["Open CI"], "review-1": [], "check-1": [] });
  assert.equal(seen.buttonsVisibleAtRest, false, "Open CI and Cancel show on hover, as Stop does");
  assert.equal(seen.opened, "panel:reviews Progress");
  assert.deepEqual(seen.calls, ["cancelMerge activities-fixture", "open_url https://github.com/o/r/actions/runs/7"]);
  assert.equal(seen.ended, true, "the checks passed, the review ended, CI passed and the merge was cancelled: no rows");
  assert.deepEqual(seen.next, { "check-1": "next", merge: "merging" });
  // Refused for want of checks: it waits on the user, Merge anyway always in sight, and merges without them.
  assert.deepEqual(seen.refused, { label: "Merge into main", state: "waiting · for you", moving: true, elapsed: null, tip: "The checks haven't run on abc1234." });
  assert.deepEqual(seen.refusedButtons, ["Merge anyway"]);
  assert.equal(seen.refusedVisibleAtRest, true);
  assert.deepEqual(seen.anyway, ["mergeSession anyway=true"]);
  assert.deepEqual(seen.merged, ["check-1"]);
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
