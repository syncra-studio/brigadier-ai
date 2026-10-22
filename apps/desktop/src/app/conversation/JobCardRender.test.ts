import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  cards: string[];
  tone: string | null;
  headline: string | null;
  detail: string[];
  buttons: string[];
  timer: boolean;
  bar: string[] | null;
  line: string[];
  spinning: boolean;
  status: string | null;
  text: string;
  schedule: string | null;
};

test("the Job card shows only for a plan or an overnight run, saying what is happening right now from the board's events", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "job-card.html", "job-card-result", 30000, { reducedMotion: true })) as Record<string, Seen> & {
    tab: { steps: string[]; current: (string | null)[]; findings: number };
  };
  const card = (id: string) => {
    const { tone, headline, detail, buttons } = seen[id]!;
    return { tone, headline, detail, buttons };
  };

  // No Job card outside a plan or an overnight run, whatever the session does: idle, at work,
  // reviews, CI, merges held, refused or done, waiting on the user, failures and outcomes,
  // and the thread's own headline. Their buttons are the Background rows'.
  for (const id of [
    "heldMerge", "mergeRefused", "ciFailed", "idle", "working", "reviewRunning", "reviewFindings", "reviewClean",
    "ciChecking", "merging", "merged", "mergeRefusedFirm", "mergeRefusedOld", "mergedAnyway", "waitingAnswer", "waitingApproval",
    "workerFailed", "devServer", "stopped", "headline",
  ]) {
    assert.deepEqual(seen[id]!.cards, ["context"], `${id} has no Job card`);
  }

  // A plan's card: the thread's headline wins, its detail after the step, and no buttons.
  assert.deepEqual(card("planHeadline"), {
    tone: "running",
    headline: "Investigating the dev server",
    detail: ["Step 1 of 3 · Looking for what holds port 1420"],
    buttons: [],
  });
  assert.deepEqual(card("planMergeRefused"), {
    tone: "waiting",
    headline: "Can’t merge yet",
    detail: ["Step 1 of 3 · The checks haven't run on 7d2a91f, the tip of brigadier/s."],
    buttons: [],
  });

  // A success never carries the failure mark: CI that GitHub marked cancelled before the thread
  // said it passed is the merge's tick; only words that tell of a failure get the cross.
  assert.equal(card("planCiCancelled").headline, "Merged, pushed, CI passes");
  assert.equal(card("planCiCancelled").tone, "merged");
  assert.deepEqual(card("planFailedHeadline"), {
    tone: "failed",
    headline: "The tests still fail",
    detail: ["Step 2 of 3 · Next: ask you how to go on"],
    buttons: [],
  });

  // No fixed phases: only a real plan has a bar, one segment per step, the step at work before the next.
  for (const [id, column] of Object.entries(seen)) {
    if (!id.startsWith("plan") && typeof column === "object" && "bar" in column) assert.equal(column.bar, null, `${id} has no bar`);
  }
  assert.deepEqual(seen["plan"]!.bar, ["active", "pending", "pending"]);
  assert.deepEqual(seen["plan"]!.line, ["active:Move settings", "next:Delete old store"]);
  assert.deepEqual(seen["plan"]!.detail, ["Step 1 of 3"]);

  // It lists nothing of the context card's: no worker row, no branch row.
  for (const [id, column] of Object.entries(seen)) {
    if (typeof column !== "object" || !("text" in column)) continue;
    assert.doesNotMatch(column.text, /Shared MCP and skills|brigadier\/web-ui/, `${id} repeats a context row`);
  }

  assert.deepEqual(seen["scheduled"]!.cards, ["context", "schedule"]);
  assert.equal(seen["scheduled"]!.schedule, "Resumes at 4:00 AMin 2h 10mwhen the Claude limit resets");
  assert.deepEqual(seen.tab.steps, ["Building", "Reviewing", "CI failed", "Fixing", "Merging"]);
  assert.deepEqual(seen.tab.current, [null, null, null, "step", null]);
  assert.equal(seen.tab.findings, 3);
});
