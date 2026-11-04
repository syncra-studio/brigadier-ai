import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const all = (names: string[]) => Object.fromEntries(names.map((name) => [name, true]));

test("the card and Source tell the current changes after every kind of change, and a read begun before a mutation never undoes it", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "source-freshness.html", "source-freshness-result", 60000)) as { error?: string; checks?: Record<string, boolean> };
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.checks, all([
    "cardShowsUncommittedCount",
    "cardRereadsAfterStage",
    "cardRereadsAfterDiscard",
    "cardRereadsAfterCommit",
    "cardRereadsAfterOutsideEdit",
    "cardRereadsAfterLanding",
    "cardRereadsOnFocus",
    "cardRereadsAfterTurnEnd",
    "olderStatDoesNotReplaceNewer",
    "burstOfReadingsStillUpdatesTheCard",
    "countRereadsOnFocusWithFilesShown",
    "countRereadsOnFocusWithSearchShown",
    "countRereadsOnFocusWithPaneClosed",
    "countRereadsAfterLandingWithPaneClosed",
    "countRereadsAfterTurnEndWithPaneClosed",
    "staleReadDoesNotUndoStage",
    "staleReadDoesNotUndoDiscard",
    "staleReadDoesNotUndoCommit",
  ]));
});

test("a draft's count and dot re-read on focus, with the pane closed or on another view", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "source-freshness.html?draft=1", "source-freshness-result", 30000)) as { error?: string; checks?: Record<string, boolean> };
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.checks, all(["draftCountRereadsOnFocusWithPaneClosed", "draftCountRereadsOnFocusWithFilesShown"]));
});
