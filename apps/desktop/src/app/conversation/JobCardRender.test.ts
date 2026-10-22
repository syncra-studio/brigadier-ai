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

test("the Job card shows only for a plan or a button, saying what is happening right now from the board's events", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "job-card.html", "job-card-result", 30000, { reducedMotion: true })) as Record<string, Seen> & {
    sent: string[];
    tab: { steps: string[]; current: (string | null)[]; findings: number };
  };
  const card = (id: string) => {
    const { tone, headline, detail, buttons } = seen[id]!;
    return { tone, headline, detail, buttons };
  };

  // No Job card for the session's state alone, outside a plan or an overnight run: idle, at work,
  // checks, reviews, merges, waiting on the user, failures and outcomes.
  for (const id of [
    "idle", "working", "checksRunning", "checksFailed", "checksPassed", "reviewRunning", "reviewFindings", "reviewClean",
    "merging", "merged", "mergedUnchecked", "mergeRefusedFirm", "mergeRefusedOld", "mergedAnyway", "waitingAnswer",
    "waitingApproval", "workerFailed", "devServer", "stopped", "nextMessage",
  ]) {
    assert.deepEqual(seen[id]!.cards, ["context"], `${id} has no Job card`);
  }

  // Only a button keeps it: CI's Open CI, the held merge's Cancel, and Merge anyway for a refusal the user can waive.
  assert.deepEqual(card("ciChecking"), { tone: "running", headline: "Checking CI · 3 of 5 jobs done", detail: ["main · 4e1c49e9"], buttons: ["Open CI"] });
  assert.deepEqual(card("ciFailed"), { tone: "failed", headline: "CI failed · test", detail: ["main · 4e1c49e9"], buttons: ["Open CI"] });
  assert.deepEqual(card("heldMerge"), {
    tone: "running",
    headline: "Merging after 1 worker finishes",
    detail: ["Into main once they’re done, as you asked"],
    buttons: ["Cancel"],
  });
  assert.deepEqual(card("mergeRefused"), {
    tone: "waiting",
    headline: "Can’t merge yet",
    detail: ["The checks haven't run on 7d2a91f, the tip of brigadier/s."],
    buttons: ["Merge anyway"],
  });
  assert.equal(seen["mergeRefused"]!.timer, false);
  assert.ok(seen.sent.includes("cancelMerge"), `Cancel cancels the held merge: ${seen.sent.join(", ")}`);
  assert.ok(seen.sent.includes("open https://github.com/o/r/actions/runs/test"), `Open CI opens the failed run: ${seen.sent.join(", ")}`);
  assert.ok(seen.sent.includes("mergeSession anyway=true"), `Merge anyway merges without the checks: ${seen.sent.join(", ")}`);

  // The thread's headline wins, its detail under it, while the held merge's Cancel keeps the card.
  assert.deepEqual(card("headline"), {
    tone: "running",
    headline: "Investigating the dev server",
    detail: ["Looking for what holds port 1420"],
    buttons: ["Cancel"],
  });

  // No fixed phases: only a real plan has a bar, one segment per step, the step at work before the next.
  for (const [id, column] of Object.entries(seen)) {
    if (id !== "plan" && typeof column === "object" && "bar" in column) assert.equal(column.bar, null, `${id} has no bar`);
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
  assert.deepEqual(seen.tab.steps, ["Building", "Reviewing", "Checks failed", "Fixing", "Merging"]);
  assert.deepEqual(seen.tab.current, [null, null, null, "step", null]);
  assert.equal(seen.tab.findings, 3);
});
