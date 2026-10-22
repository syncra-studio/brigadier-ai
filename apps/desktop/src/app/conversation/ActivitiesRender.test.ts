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

type Ended = { state: string; icon: string | null; red: boolean; shimmer: boolean; muted: boolean; lastLine: string | null; elapsed: string; actions: string[]; actionsVisibleAtRest: boolean | null; clear?: boolean | null; cut?: boolean | null };

test("Background rows stay while they need the user (Q94): a failed one red with its last line, Restart and × clear of its words; a finished or stopped one gone", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?ended=1", "activities-result", 60000)) as {
    order: string[]; rows: Record<string, Ended>; calls: string[]; restarted: string; afterActions: string[]; startedAgain: string[];
  };
  // Newest first; the docs build and lint that finished and the storybook the user stopped are not there.
  assert.deepEqual(seen.order, ["preview-12", "preview-11", "run-13", "run-15"]);
  // The name and its last line are long enough, at the card's width, to need cutting off (#511):
  // the words end in an ellipsis before Restart and ×, never under them.
  assert.deepEqual(seen.rows["preview-11"], { state: "failed", icon: "failed", red: true, shimmer: false, muted: false,
    lastLine: "· Error: the bundle couldn't be opened, check the build log for details", elapsed: "1m",
    actions: ["Restart Brigadier dev app", "Dismiss Brigadier dev app"], actionsVisibleAtRest: true, clear: true, cut: true });
  // A crash is a failure too, and so is a run with a non-zero exit or one cut off.
  assert.equal(seen.rows["preview-12"]?.state, "failed");
  assert.deepEqual([seen.rows["run-13"]?.state, seen.rows["run-13"]?.lastLine, seen.rows["run-13"]?.actions, seen.rows["run-13"]?.actionsVisibleAtRest],
    ["failed", "· test result: FAILED. 9 passed; 3 failed", ["Restart Core tests", "Dismiss Core tests"], true]);
  assert.equal(seen.rows["run-15"]?.state, "failed");
  assert.equal(seen.rows["run-15"]?.actionsVisibleAtRest, true, "a failed row's Restart and × show without hover");
  // Every failed row's words stay clear of its Restart and ×, not only the longest one.
  for (const activity of ["preview-11", "preview-12", "run-13", "run-15"]) assert.equal(seen.rows[activity]?.clear, true, activity);
  assert.deepEqual(seen.calls, ["dismissBackground run-15", "restartBackgroundRun run-13", "restartPreview preview-11"]);
  // Restarted under its id, the page is its running row again; the run restarted and the one dismissed left.
  assert.equal(seen.restarted, "starting");
  assert.deepEqual(seen.afterActions, ["preview-11", "preview-12"]);
  // The thread starts the api server again: the new one replaces the failed row.
  assert.deepEqual(seen.startedAgain, ["preview-15", "preview-11"]);
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
