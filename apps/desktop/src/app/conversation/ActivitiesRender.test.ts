import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Row = { label: string; meta: string; elapsed: string; state: string } | null;

test("Activities shows each thing running for the session as a row, the dev app one row from build to app, with Stop, Open and a folded count", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "activities.html?drive=1", "activities-result", 60000)) as {
    empty: boolean; order: string[]; rows: Record<string, Row>; workerLine: string | null;
    starting: Row; running: Row; sameRow: boolean;
    appLine: string | null; appButtons: string[]; pageButtons: string[]; runButtons: string[];
    opened: string | null; previews: boolean; calls: string[]; folded: string | null;
  };
  assert.equal(seen.empty, true, "nothing running, no section");
  // Running first, newest first: the previews and the thread's long commands in one list.
  assert.deepEqual(seen.order, ["run-1", "run-2", "preview-2", "preview-1"]);
  assert.deepEqual(seen.rows["preview-1"], { label: "Demo pages", meta: "Showing", elapsed: "12m", state: "working" });
  assert.deepEqual(seen.rows["preview-2"], { label: "Dev app", meta: "Building", elapsed: "4m", state: "working" });
  assert.deepEqual(seen.rows["run-1"], { label: "Core tests", meta: "Running", elapsed: "2m", state: "working" });
  // A command the thread gave no name reads as its command, cut short.
  assert.equal(seen.rows["run-2"]?.label, "pnpm lint --max-warnings 0 --cache --ca…");
  // The worker's other work still shows on its own line.
  assert.match(seen.workerLine ?? "", /cargo clippy --workspace/);
  assert.deepEqual(seen.starting, { label: "Dev app", meta: "Starting", elapsed: "4m", state: "working" });
  assert.deepEqual(seen.running, { label: "Dev app", meta: "Running", elapsed: "4m", state: "working" });
  assert.equal(seen.sameRow, true, "the build and the app are the same row");
  // Opened: one command line naming the worker, and Stop; Open only for a page that is up.
  assert.match(seen.appLine ?? "", /^Demo of the fixes ·\s*pnpm tauri build --debug --bundles app/);
  assert.deepEqual(seen.appButtons, ["Stop"]);
  assert.deepEqual(seen.pageButtons, ["Open", "Stop"]);
  assert.deepEqual(seen.runButtons, ["Stop"]);
  assert.equal(seen.opened, "http://localhost:5199/");
  assert.equal(seen.previews, true, "the command line opens the previews and their logs");
  assert.deepEqual(seen.calls, ["stopBackgroundRun run-1", "stopPreview preview-1"]);
  assert.match(seen.folded ?? "", /^Activities\s*· 4 running/);
});
