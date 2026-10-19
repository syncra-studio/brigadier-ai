import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Row = { label: string; elapsed: string; tip: string } | null;

test("Background lists what still runs, one line each with Stop on hover; a web preview opens in the browser, the rest open their output in a tab, and what ends leaves the card", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?drive=1", "activities-result", 60000)) as {
    empty: boolean; title: string; order: string[]; rows: Record<string, Row>; heights: number[]; workerLine: string | null; sameRow: boolean;
    stops: string[]; browsed: string | null; outputs: boolean[]; opened: string | null; openedApp: string; openedRun: string; grew: boolean; calls: string[]; left: string[];
  };
  assert.equal(seen.empty, true, "nothing running, no section");
  assert.equal(seen.title, "Background");
  // Newest first: the previews and the thread's long commands in one list.
  assert.deepEqual(seen.order, ["run-1", "run-2", "preview-2", "preview-1"]);
  assert.deepEqual(seen.rows["preview-1"], { label: "Demo pages", elapsed: "12m", tip: "Showing: pnpm dev --port 5199" });
  assert.equal(seen.rows["preview-2"]?.label, "Dev app");
  assert.match(seen.rows["preview-2"]?.tip ?? "", /^Building: pnpm tauri build[^]*\nFrom Demo of the fixes$/);
  assert.deepEqual(seen.rows["run-1"], { label: "Core tests", elapsed: "2m", tip: "cargo test -p brigadier-core" });
  // A command the thread gave no name reads as its command, cut short.
  assert.equal(seen.rows["run-2"]?.label, "pnpm lint --max-warnings 0 --cache --ca…");
  assert.deepEqual(seen.heights, [28], "one line each");
  // The worker's line names it; what it runs shows in the Workers panel, not the card.
  assert.doesNotMatch(seen.workerLine ?? "", /cargo clippy --workspace/);
  assert.equal(seen.sameRow, true, "the build and the app are the same row");
  assert.deepEqual(seen.stops, ["Stop", "Stop", "Stop", "Stop"]);
  // The web preview opens in Brigadier's browser; its output is its row's Show output.
  assert.equal(seen.browsed, "http://localhost:5199/");
  assert.deepEqual(seen.outputs, [true, false, false]);
  assert.equal(seen.opened, "panel:output:preview:preview-1 Demo pages");
  // An app preview and a command open what they print.
  assert.equal(seen.openedApp, "panel:output:preview:preview-2");
  assert.equal(seen.openedRun, "panel:output:run:run-1");
  assert.equal(seen.grew, false, "nothing opens in the card");
  assert.deepEqual(seen.calls, ["stopBackgroundRun run-1", "stopPreview preview-1"]);
  assert.deepEqual(seen.left, ["run-2", "preview-2"], "what ended left the card");
});
