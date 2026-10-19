import assert from "node:assert/strict";
import { test } from "node:test";

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
  cases: Record<"merged-finding" | "collapsed" | "all" | "confirm" | "reverting" | "reverted" | "branch" | "merge-only", Look>;
  row: unknown;
  viewChanges: unknown;
  finding: string | null;
  branchRow: unknown;
  cancelled: string[];
  revertedOpen: number;
  sent: string[];
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
  // Reverted: one folded line, the counts dim, and its files a click away.
  assert.deepEqual(
    [cases.reverted.state, cases.reverted.title, cases.reverted.status, cases.reverted.files, cases.reverted.buttons],
    ["reverted", "Reverted 6 files", "Undone on main", [], ["Show the files"]],
  );
  assert.equal(seen.revertedOpen, 3);
  // Not merged yet: on its branch, Discard and Merge into main.
  assert.deepEqual([cases.branch.state, cases.branch.status, cases.branch.buttons], ["branch", "On three-small-improvements · 2 commits ahead", ["Discard", "Merge into main", "Fold the files"]]);
  // A merge with nothing of its own counted: the merge's card.
  assert.deepEqual([cases["merge-only"].title, cases["merge-only"].status, cases["merge-only"].buttons], ["Merged into main", "2 commits from brigadier/due-dates", ["Revert", "View changes"]]);
  // A row opens that file's part of the merge in a tab; View changes all of it; Open finding the Progress tab.
  assert.deepEqual(seen.row, { id: "review:f1f1f1f1", target: { type: "commit", commit: "f1f1f1f1", staged: false, path: "public/index.html" } });
  assert.deepEqual(seen.viewChanges, { id: "review:f1f1f1f1", target: { type: "commit", commit: "f1f1f1f1", staged: false } });
  assert.equal(seen.finding, "panel:reviews");
  // Not merged, a row opens its part of the turn.
  assert.deepEqual(seen.branchRow, { id: "review", target: { type: "lastTurn", requestId: "branch", path: "public/index.html", staged: false } });
  // Revert and Merge ask the thread, once each.
  assert.deepEqual(seen.sent, ["Revert merge c1c1c1c on main the way it went in", "Merge three-small-improvements into main"]);
});
