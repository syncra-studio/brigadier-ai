import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Row = { label: string; state: string | null; moving: boolean; elapsed: string; tip: string } | null;

test("Background lists what still runs, one line each with its state and Stop on hover; a web preview opens in the browser, another preview opens its output in a tab, and what the user stopped leaves the card", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?drive=1", "activities-result", 60000)) as {
    empty: boolean; title: string; order: string[]; rows: Record<string, Row>; heights: number[]; workerLine: string | null; sameRow: boolean;
    stops: string[]; stopVisibleAtRest: boolean; browsed: string | null; outputs: boolean[]; opened: string | null; openedApp: string; grew: boolean; calls: string[]; left: string[];
  };
  assert.equal(seen.empty, true, "nothing running, no section");
  assert.equal(seen.title, "Background");
  // Newest first.
  assert.deepEqual(seen.order, ["preview-2", "preview-1"]);
  assert.deepEqual(seen.rows["preview-1"], { label: "Demo pages", state: null, moving: false, elapsed: "12m", tip: "Showing: pnpm dev --port 5199" });
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
  assert.match(seen.opened ?? "", /^panel:output:preview:preview-1:\d+ Demo pages$/);
  // An app preview opens what it prints.
  assert.match(seen.openedApp, /^panel:output:preview:preview-2:\d+$/);
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
  assert.equal(seen.section, true, "History remains after all previews end");
  assert.equal(seen.restartOrDismiss, 0);
});

type Look = { word: string | null; icon: string | null; moving: boolean; green: boolean; shimmer: boolean; sweeping: boolean };

test("Background rows: a spinner while a process is on its way, a green live dot once it is up, a pause sign while held; the word shimmers while it works, and nothing moves with less motion", { timeout: 120000 }, async (t) => {
  const looks = JSON.parse(await renderFixtureLive(t, "activities.html?states=1", "activities-result", 60000)) as Record<string, Look>;
  const busy = { icon: "spinner", moving: true, green: false, shimmer: true, sweeping: true };
  const expected: Record<string, Look> = {
    building: { ...busy, word: "building" },
    starting: { ...busy, word: "starting" },
    "smoke testing": { ...busy, word: "smoke testing" },
    checking: { ...busy, word: "checking" },
    running: { word: null, icon: "live", moving: true, green: true, shimmer: false, sweeping: false },
    paused: { word: "paused", icon: "paused", moving: false, green: false, shimmer: false, sweeping: false },
  };
  assert.deepEqual(looks, expected);
  const still = JSON.parse(await renderFixtureLive(t, "activities.html?states=1", "activities-result", 60000, { reducedMotion: true })) as Record<string, Look>;
  for (const [state, look] of Object.entries(still)) {
    assert.deepEqual([look.moving, look.sweeping], [false, false], `${state} is still with less motion`);
    assert.equal(look.icon, expected[state]?.icon, `${state} keeps its icon`);
  }
});

test("commands leave Background for live strips while History remains after they finish", { timeout: 90000 }, async (t) => {
  for (const scenario of ["tasks", "workerTasks"]) {
    const seen = JSON.parse(await renderFixtureLive(t, `activities.html?${scenario}=1`, "activities-result", 60000)) as { running: string[]; ended: string[]; history: boolean };
    assert.deepEqual(seen.running, []);
    assert.deepEqual(seen.ended, []);
    assert.equal(seen.history, true);
  }
});

test("only Background names truncate: building stays whole and shimmering, running needs only its dot, and short names keep the word beside them", { timeout: 90000 }, async (t) => {
  type Name = { truncated: boolean; ellipsis: string; state: string | null; stateFits: boolean; gap: number | null; shimmer: boolean; sweeping: boolean; green: boolean; height: number; beforeTime: boolean };
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?names=1", "activities-result", 60000)) as {
    "800": Record<string, Name>; "420": Record<string, Name>; hover: Name & { stopVisible: boolean };
  };
  for (const width of ["800", "420"] as const) {
    const names = seen[width];
    const building = names["long-building"]!;
    assert.equal(building.truncated, true, `${width}: the long building name truncates`);
    assert.equal(building.ellipsis, "ellipsis");
    assert.equal(building.state, "building");
    assert.equal(building.stateFits, true, `${width}: the entire building word fits`);
    assert.equal(building.shimmer && building.sweeping, true);
    const short = names["preview-2"]!;
    assert.equal(short.truncated, false);
    assert.equal(short.stateFits, true);
    assert.ok(short.gap !== null && short.gap > 0 && short.gap <= 8, `${width}: the state sits right after the short name`);
    for (const activity of ["long-running"]) {
      const running = names[activity]!;
      assert.equal(running.truncated, true, `${width}: ${activity} truncates its name`);
      assert.equal(running.ellipsis, "ellipsis");
      assert.equal(running.state, null, `${width}: ${activity} has no running word`);
      assert.equal(running.green, true, `${width}: ${activity} keeps its live dot`);
    }
    for (const name of Object.values(names)) {
      assert.equal(name.height, 28, `${width}: rows stay one line high`);
      assert.equal(name.beforeTime, true, `${width}: names and state words leave room for time`);
    }
  }
  assert.equal(seen.hover.stopVisible, true);
  assert.equal(seen.hover.truncated && seen.hover.stateFits && seen.hover.shimmer, true, "hover leaves room for the whole building word and Stop");
});


test("raw session/worker apps use Background once and stop only their exact native invocation", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?apps=1", "activities-result", 60000)) as {
    running: string[]; checks: string[]; workerName: string; output: string; ended: string[]; history: boolean;
    stops: { id: string; worker: string | null; started: number }[]; expectedStarts: number[];
  };
  assert.deepEqual(seen.running, ["task:raw-browser", "task:raw-dev", "task:task-2:b7dev"]);
  assert.deepEqual(seen.checks, ["raw-tests"], "native test command stays Checks despite a misleading dev label");
  assert.equal(seen.workerName, "Demo of the fixes");
  assert.match(seen.output, /^panel:output:task:raw-browser:\d+$/, "terminal output remains invocation-bound");
  assert.deepEqual(seen.ended, [], "completed, failed and stopped apps leave immediately");
  assert.equal(seen.history, true);
  assert.deepEqual(seen.stops, [
    { id: "raw-dev", worker: null, started: seen.expectedStarts[0] },
    { id: "b7dev", worker: "task-2", started: seen.expectedStarts[1] },
    { id: "raw-dev", worker: null, started: seen.expectedStarts[2] },
  ]);
});
