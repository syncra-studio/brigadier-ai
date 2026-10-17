import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// While its request runs, the thread's feedback offer waits: only the finished request's shows.
test("a feedback offer never shows mid-turn", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.cards, ["Typo fixes take long"]);
});

// A turn the user stopped has ended too: its offer shows, to answer.
test("a feedback offer shows after a stopped turn", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1&stopped=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.cards, ["Typo fixes take long", "Stop leaves the tab spinning"]);
});

// Once the turn ends: the draft with #146's details, sent or edited first through Send feedback,
// or dismissed for good; each answer goes to the daemon.
test("a feedback offer sends, edits first or dismisses", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1&done=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.cards, ["Typo fixes take long", "Stop leaves the tab spinning"]);
  assert.equal(seen.details, "Adds Brigadier 0.1.0, macOS 27.0 Tahoe (aarch64), Claude Code 2.1.4");
  assert.deepEqual(seen.buttons, ["Open on GitHub", "Edit first", "Dismiss"]);
  const body = "After I press Stop mid-reply, the tab keeps its spinner.";
  assert.deepEqual(seen.edit, { text: `Stop leaves the tab spinning\n\n${body}`, kind: "Something’s wrong" });
  assert.deepEqual(seen.opened, [
    {
      template: "bug_report.yml",
      title: "[Bug]: Stop leaves the tab spinning",
      actual: `Stop leaves the tab spinning\n\n${body}`,
      version: "0.1.0",
      environment: "macOS 27.0 Tahoe (aarch64), Claude Code 2.1.4",
    },
  ]);
  assert.equal(seen.sent, "Opened on GitHub: finish it there and post it.");
  assert.equal(seen.left, 1);
  assert.deepEqual(seen.answers, [
    { method: "answerFeedbackOffer", conversationId: (seen.answers as { conversationId: string }[])[0]?.conversationId, offerId: "second", sent: true },
    { method: "answerFeedbackOffer", conversationId: (seen.answers as { conversationId: string }[])[0]?.conversationId, offerId: "first", sent: false },
  ]);
  assert.equal(seen.dialogOpen, false);
});
