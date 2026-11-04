import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("tabs of one name say their checkout only while they clash; the tab in front's row lights up in its own checkout", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "tab-names-rows.html?drive=1", "tab-names-rows-result", 45000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.oneTab, { tabs: ["main.ts"], notes: [] });
  assert.deepEqual(seen.clash, { tabs: ["main.tsbrigadier-tabs", "main.tsbrigadier-tabs-mine"], notes: ["brigadier-tabs", "brigadier-tabs-mine"] });
  assert.equal(seen.noteDim, true, "the note is dimmer than the name");
  assert.deepEqual(seen.clashWithOther, { tabs: ["main.tsbrigadier-tabs", "main.tsbrigadier-tabs-mine", "README.md"], notes: ["brigadier-tabs", "brigadier-tabs-mine"] });
  assert.deepEqual(seen.clashOver, { tabs: ["main.ts"], notes: [] });
  // Stacked rows: the project's folder first, the worktree after it.
  assert.deepEqual(seen.folderFile, [["src/main.ts"], []]);
  assert.deepEqual(seen.worktreeFile, [[], ["src/main.ts"]]);
  assert.deepEqual(seen.worktreeChange, [[], ["src/main.ts"]]);
  assert.deepEqual(seen.stagedChange, [["README.md"], []]);
  assert.deepEqual(seen.unstagedOfStaged, [[], []], "a change's Review lights up only its own side's row");
  assert.deepEqual(seen.wholeGroup, [[], []]);
  assert.deepEqual(seen.chatInFront, [[], []]);
  assert.deepEqual(seen.draftFiles, ["only-in-folder.txt · Untracked"]);
  assert.equal(seen.litBackground, true);
  assert.deepEqual(seen.draftFilesWorktreeTab, [], "the project's tree doesn't light up for a worktree's tab");
  assert.deepEqual(seen.sessionSource, [["src/main.ts"]]);
  assert.deepEqual(seen.sessionFiles, ["README.md · Modified"]);
  assert.deepEqual(seen.sameCheckout, ["src/a", "src/b"], "within one checkout the file's folder tells them apart");
});
