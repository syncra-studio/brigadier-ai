import assert from "node:assert/strict";
import { test } from "node:test";

import type { FoldSeen } from "@/fixtures/cardFold";
import { renderFixtureLive } from "@/fixtures/headless";

type Look = {
  state: string | null;
  title: string | null;
  counts: string | null;
  status: string | null;
  problem: string | null;
  buttons: string[];
  files: [string, string, string][];
  more: string | null;
  warning: [string, string, string] | null;
  confirm: string | null;
};
type Seen = {
  error?: string;
  cases: Record<"merged-finding" | "collapsed" | "all" | "confirm" | "reverting" | "reverted" | "thread-reverted" | "branch" | "merge-only" | "waiting" | "reviewed", Look>;
  measured: {
    confirmGap: number | null;
    revertingInset: number | null;
    counts: string[];
    reviewBeforeCounts: boolean;
    focused: { shown: string | null; washed: boolean };
  };
  row: unknown;
  viewChanges: unknown;
  finding: string | null;
  branchRow: unknown;
  cancelled: string[];
  revertedOpen: number;
  sent: string[];
  foldedRevert: [string | null, boolean];
};

const FIRST = [["public/", "index.html", "+11 −2"], ["src/", "server.js", "+22 −5"], ["src/", "store.js", "+5 −0"]];

// The Changes card in each state of the user's design (Of3wE).
test("the Changes card: merged with a finding, folded, all files, Revert's question, reverting, reverted, not merged, a merge alone", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "changes-card.html?drive=1", "changes-card-result", 60000)) as Seen;
  assert.equal(seen.error, undefined);
  const { cases } = seen;
  // Merged, an open finding: three files, then Show more; the finding is the footer row, named.
  const finding = cases["merged-finding"];
  assert.deepEqual([finding.state, finding.title, finding.counts, finding.status, finding.problem], ["merged", "Edited 6 files", "+88 −7", "Merged into main", null]);
  assert.deepEqual(finding.buttons, ["Revert", "View changes", "Fold the files"]);
  assert.deepEqual(finding.files, FIRST);
  assert.equal(finding.more, "Show 3 more files");
  assert.deepEqual(finding.warning, ["One review finding is still open", "R4.1 · A malformed request path can crash the server", "Open finding"]);
  // Folded: just the header; the finding moves into its line, never hidden.
  assert.deepEqual([cases.collapsed.files, cases.collapsed.warning, cases.collapsed.problem], [[], null, "1 open finding"]);
  assert.deepEqual(cases.collapsed.buttons, ["Revert", "View changes", "Show the files"]);
  // Every file, and no footer with nothing wrong.
  assert.deepEqual([cases.all.files.length, cases.all.more, cases.all.warning], [6, "Show less", null]);
  // Revert asks in the footer first; sent, the footer reads Reverting….
  assert.equal(cases.confirm.confirm, "Revert these 6 files on main?Adds one commit that undoes +88 −7. Work merged after this stays.CancelRevert 6 files");
  assert.deepEqual(cases.confirm.buttons, ["View changes", "Fold the files"]);
  assert.match(cases.reverting.confirm ?? "", /Reverting…$/);
  assert.deepEqual(seen.cancelled, ["Revert", "View changes", "Fold the files"]);
  // Reverted: one folded line, when git shows the revert, Redo, and its files a click away.
  assert.deepEqual(
    [cases.reverted.state, cases.reverted.title, cases.reverted.status, cases.reverted.files, cases.reverted.buttons],
    ["reverted", "Reverted 6 files", "Undone on main · 4:02 PM", [], ["Redo", "Show the files"]],
  );
  assert.equal(seen.revertedOpen, 3);
  // Revert on a folded card opens it to its question, never a question hidden in the fold.
  assert.deepEqual(seen.foldedRevert, ["Revert these 6 files on main?Adds one commit that undoes +88 −7. Work merged after this stays.CancelRevert 6 files", true]);
  // Reverted through the thread (a later merge, the session reads Merged): git's revert says so.
  const thread = cases["thread-reverted"];
  assert.deepEqual([thread.state, thread.title, thread.status, thread.buttons], ["reverted", "Reverted 6 files", "Undone on main · 4:02 PM", ["Redo", "Show the files"]]);
  // Not merged yet: on its branch, Discard and Merge into main.
  assert.deepEqual([cases.branch.state, cases.branch.status, cases.branch.buttons], ["branch", "On three-small-improvements · 2 commits ahead", ["Discard", "Merge into main", "Fold the files"]]);
  // A merge with nothing of its own counted: the merge's card.
  assert.deepEqual([cases["merge-only"].title, cases["merge-only"].status, cases["merge-only"].buttons], ["Merged into main", "2 commits from brigadier/due-dates", ["Revert", "View changes"]]);
  // Waiting for review: Undo All, Keep All, Review Changes. Reviewed: Discard and Merge again.
  assert.deepEqual([cases.waiting.title, cases.waiting.status, cases.waiting.buttons], ["6 Files", "3 to review", ["Undo All", "Keep All", "Review Changes", "Fold the files"]]);
  assert.deepEqual(
    [cases.reviewed.status, cases.reviewed.buttons],
    ["kept 2, undid 1 · On three-small-improvements · 2 commits ahead", ["Discard", "Merge into main", "Fold the files"]],
  );
  // The design's measures: Cancel and Revert 4 apart, "Reverting…" 12 in, counts right-aligned, a
  // waiting row's Undo and Keep before its counts, shown on the washed row while it has focus.
  assert.deepEqual(seen.measured, { confirmGap: 4, revertingInset: 12, counts: ["end"], reviewBeforeCounts: true, focused: { shown: "flex", washed: true } });
  // A row opens that file's part of the merge in a tab; View changes all of it; Open finding the Progress tab.
  assert.deepEqual(seen.row, { id: "review:f1f1f1f1", target: { type: "commit", commit: "f1f1f1f1", staged: false, path: "public/index.html" } });
  assert.deepEqual(seen.viewChanges, { id: "review:f1f1f1f1", target: { type: "commit", commit: "f1f1f1f1", staged: false } });
  assert.equal(seen.finding, "panel:reviews");
  // Not merged, a row opens its part of the turn.
  assert.deepEqual(seen.branchRow, { id: "review", target: { type: "lastTurn", requestId: "branch", path: "public/index.html", staged: false } });
  // Revert, Merge and Redo ask the thread, once each.
  assert.deepEqual(seen.sent, [
    "Revert merge c1c1c1c on main the way it went in",
    "Merge three-small-improvements into main",
    "Redo merge e1e1e1e on main: revert its revert d0d0d0d the way it went in",
  ]);
});

// The whole header folds and opens the card, its buttons keep their own action, and the body's
// height and opacity animate, at once with reduced motion.
for (const reducedMotion of [false, true]) {
  test(`a click on the header folds and opens the card${reducedMotion ? ", at once with reduced motion" : ", animated"}`, { timeout: 120000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, "changes-card.html?case=all&drive=fold", "changes-card-result", 60000, { reducedMotion })) as FoldSeen & { error?: string };
    assert.equal(seen.error, undefined);
    // View changes does its own thing; the card stays open.
    assert.equal(seen.afterButton, 6);
    assert.deepEqual(seen.folding, { state: "closed", animation: reducedMotion ? "none" : "collapsible-up" });
    assert.deepEqual(seen.unfolding, { state: "open", animation: reducedMotion ? "none" : "collapsible-down" });
    assert.equal(seen.reopened, 6);
    assert.notEqual(seen.closedWithinMs, null);
    if (reducedMotion) assert.ok(seen.closedWithinMs! < 100, `closed after ${seen.closedWithinMs} ms`);
    else assert.ok(seen.closedWithinMs! >= 150, `closed after ${seen.closedWithinMs} ms, not animated`);
  });
}
