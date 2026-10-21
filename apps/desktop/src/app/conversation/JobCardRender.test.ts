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

test("the Job card says what is happening right now, from the board's events, and is gone when nothing runs or waits", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "job-card.html", "job-card-result", 30000, { reducedMotion: true })) as Record<string, Seen> & {
    sent: string[];
    tab: { steps: string[]; current: (string | null)[]; findings: number };
  };
  const card = (id: string) => {
    const { tone, headline, detail, buttons } = seen[id]!;
    return { tone, headline, detail, buttons };
  };

  // Idle: no Job card, the context card only (J4).
  assert.deepEqual(seen["idle"]!.cards, ["context"]);
  // The thread at work with nothing more to say: Working, with its timer, naming no worker.
  assert.deepEqual(card("working"), { tone: "running", headline: "Working", detail: [], buttons: [] });
  assert.equal(seen["working"]!.timer, true);
  assert.equal(seen["working"]!.spinning, true);

  // Checks (run_checks): running with the command at work, failed with what failed, passed on the commit.
  assert.deepEqual(card("checksRunning"), { tone: "running", headline: "Running checks · 3 of 5", detail: ["Now: desktop tests"], buttons: [] });
  assert.deepEqual(card("checksFailed"), { tone: "failed", headline: "Checks failed", detail: ["pnpm test: 2 tests failed"], buttons: [] });
  assert.equal(seen["checksFailed"]!.spinning, false);
  assert.deepEqual(card("checksPassed"), { tone: "done", headline: "Checks passed on 4e1c49e", detail: ["5 of 5 passed"], buttons: [] });

  // Review (request_review): who reviews, then what came back.
  assert.deepEqual(card("reviewRunning"), { tone: "running", headline: "Codex is reviewing", detail: ["The changes up to 4e1c49e"], buttons: [] });
  assert.deepEqual(card("reviewFindings"), { tone: "done", headline: "Review found 3 issues", detail: ["From Codex’s review of 4e1c49e"], buttons: [] });
  assert.equal(seen["reviewClean"]!.headline, "Review found nothing to fix");

  // CI and the held merge left the composer rail: they are the card's, buttons included (R2).
  assert.deepEqual(card("ciChecking"), { tone: "running", headline: "Checking CI · 3 of 5 jobs done", detail: ["main · 4e1c49e9"], buttons: ["Open CI"] });
  assert.deepEqual(card("ciFailed"), { tone: "failed", headline: "CI failed · test", detail: ["main · 4e1c49e9"], buttons: ["Open CI"] });
  assert.deepEqual(card("heldMerge"), {
    tone: "running",
    headline: "Merging after 1 worker finishes",
    detail: ["Into main once they’re done, as you asked"],
    buttons: ["Cancel"],
  });
  assert.ok(seen.sent.includes("cancelMerge"), `Cancel cancels the held merge: ${seen.sent.join(", ")}`);
  assert.ok(seen.sent.includes("open https://github.com/o/r/actions/runs/test"), `Open CI opens the failed run: ${seen.sent.join(", ")}`);

  // Merge: merging, then merged, saying checks passed only when they passed on the merged commit.
  assert.deepEqual(card("merging"), { tone: "running", headline: "Merging into main", detail: ["Checks passed on head111"], buttons: [] });
  assert.deepEqual(card("merged"), { tone: "merged", headline: "Merged into main · checks passed", detail: ["4e1c49e · 4 commits"], buttons: [] });
  assert.equal(seen["mergedUnchecked"]!.headline, "Merged into main");

  // Waiting on you: the card points at the card below, never copies its words or buttons (R3).
  assert.deepEqual(card("waitingAnswer"), { tone: "waiting", headline: "Waiting for your answer below", detail: [], buttons: [] });
  assert.doesNotMatch(seen["waitingAnswer"]!.text, /Build the page/);
  assert.equal(seen["waitingAnswer"]!.timer, false);
  assert.deepEqual(card("waitingApproval"), { tone: "waiting", headline: "Waiting for your approval below", detail: [], buttons: [] });

  // Failures in the work itself.
  assert.deepEqual(card("workerFailed"), { tone: "failed", headline: "A worker failed", detail: ["The registry tests failed"], buttons: [] });
  assert.deepEqual(card("devServer"), { tone: "failed", headline: "The dev server stopped", detail: ["Port 1420 is already in use"], buttons: [] });

  // A finished outcome stays until the next message; then the card is gone.
  assert.deepEqual(card("stopped"), { tone: "stopped", headline: "Stopped", detail: ["You stopped it"], buttons: [] });
  assert.deepEqual(seen["nextMessage"]!.cards, ["context"]);

  // The thread's headline wins, its detail under it; the held merge's Cancel stays reachable.
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
