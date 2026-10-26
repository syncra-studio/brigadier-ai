import assert from "node:assert/strict";
import { test } from "node:test";
import type { FoldSeen } from "@/fixtures/cardFold";
import { renderFixtureLive } from "@/fixtures/headless";

test("snapshot card keeps file rows and counts, groups merges, and confirms later files before Revert", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "changes-card.html?drive=1", "changes-card-result", 60000)) as {
    error?: string;
    before: { counts: string; rows: number; empty: number; legacy: boolean; binary: string; workerCounts: string; group: string };
    target: unknown; confirm: string; writesBeforeConfirm: number; cancelled: boolean; reverting: string;
    call: unknown; finalTitle: string;
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.before.counts, "+88 −7");
  assert.equal(seen.before.rows, 3);
  assert.equal(seen.before.empty, 0);
  assert.equal(seen.before.legacy, false);
  assert.match(seen.before.binary, /binary/);
  assert.equal(seen.before.workerCounts, "+11 −2");
  assert.match(seen.before.group, /worker\/one/);
  assert.match(seen.before.group, /\+9 −1/);
  assert.deepEqual(seen.target, { type: "lastTurn", requestId: "own", path: "public/index.html", staged: false });
  assert.match(seen.confirm, /Restore the files from the start of this turn/);
  assert.match(seen.confirm, /later.txt, src\/user-edit.ts/);
  assert.equal(seen.writesBeforeConfirm, 0);
  assert.equal(seen.cancelled, true);
  assert.match(seen.reverting, /Reverting…/);
  assert.deepEqual(seen.call, { method: "revertTurn", conversationId: "changes-card", requestId: "own", expectedTree: "confirmed-tree" });
  assert.equal(seen.finalTitle, "Reverted 6 files");
});

for (const reducedMotion of [false, true]) {
  test(`many merged changes stay bounded and keep expansion after remount (${reducedMotion ? "reduced motion" : "animation"})`, { timeout: 120000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, "changes-card.html?case=stress&drive=stress", "changes-card-result", 60000, { reducedMotion })) as {
      error?: string;
      initial: { ownRows: number; groups: number; groupRows: number; label: string; tooltip: string; labels: string[]; more: string };
      opened: { rows: number; expanded: string }; target: unknown;
      allFiles: number; fewerFiles: number; uniqueGroups: number; restoredHidden: number;
      remounted: { ownRows: number; groups: number; firstRows: number; laterRows: number };
      foldedRows: number; ownLess: number; overflow: boolean;
    };
    assert.equal(seen.error, undefined);
    assert.deepEqual(seen.initial, { ownRows: 3, groups: 3, groupRows: 0, label: "Merged changes · 93773efb", tooltip: "93773efb8b60c162861c2fa748047fd2ba769b45", labels: ["Merged changes · 93773efb", "Review session titles", "worker/ambiguous"], more: "Show 3 more merges" });
    assert.deepEqual(seen.opened, { rows: 3, expanded: "true" });
    assert.deepEqual(seen.target, { type: "commit", commit: "a".repeat(40), path: "apps/desktop/src/app/conversation/activity/session-titles/ChangedFile1.tsx", staged: false });
    assert.equal(seen.allFiles, 49);
    assert.equal(seen.fewerFiles, 3);
    assert.equal(seen.uniqueGroups, 6);
    assert.equal(seen.restoredHidden, 5);
    assert.deepEqual(seen.remounted, { ownRows: 49, groups: 6, firstRows: 3, laterRows: 5 });
    assert.equal(seen.foldedRows, 0);
    assert.equal(seen.ownLess, 3);
    assert.equal(seen.overflow, false);
  });

  test(`the snapshot card folds with the existing ${reducedMotion ? "reduced motion" : "animation"}`, { timeout: 120000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, "changes-card.html?case=all&drive=fold", "changes-card-result", 60000, { reducedMotion })) as FoldSeen & { error?: string };
    assert.equal(seen.error, undefined);
    assert.equal(seen.afterButton, 6);
    assert.deepEqual(seen.folding, { state: "closed", animation: reducedMotion ? "none" : "collapsible-up" });
    assert.deepEqual(seen.unfolding, { state: "open", animation: reducedMotion ? "none" : "collapsible-down" });
    assert.equal(seen.reopened, 6);
    assert.notEqual(seen.closedWithinMs, null);
  });
}
