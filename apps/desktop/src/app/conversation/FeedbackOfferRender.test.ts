import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// While its request runs, the thread's feedback offer waits: only the finished request's shows.
test("a feedback offer never shows mid-turn", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  // No writer for this one: its own text is its one report.
  assert.deepEqual(seen.cards, [["Typo fixes take longBug"]]);
});

const REPORTS = [
  "Usage popover doesn't refresh when a session endsBug",
  "Stop doesn't stop a running workerAlready reported",
  "Pin a chat to the top of the sidebarIdea",
];

// A turn the user stopped has ended too: its offer shows, to answer.
test("a feedback offer shows after a stopped turn", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1&stopped=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.cards, [["Typo fixes take longBug"], REPORTS]);
});

// Once the turn ends the card writes the offer up as Send feedback does (one report per bug or
// idea, a repeat marked), then sends them (failing once: Try again), edits them in Send feedback
// or is dismissed for good; each answer goes to the daemon.
test("a feedback offer is written up, then sends, edits or dismisses", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1&done=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  const working = ["Writing up your reports…", "Written from this chat · posts as @stephen-golban", "Splitting your note and checking GitHub for ones already reported", 0];
  assert.deepEqual(seen.writing, [working, working]);
  assert.deepEqual(seen.cards, [["Typo fixes take longBug"], REPORTS]);
  assert.deepEqual(seen.head, ["Send these to Brigadier’s developers?", "3 reports written from this chat · posts as @stephen-golban"]);
  assert.deepEqual(seen.buttons, ["Send", "Edit", "Dismiss"]);
  // Opened, a report is the write-up as it will be posted.
  assert.deepEqual(seen.report, [
    "What happened: the usage popover keeps showing the old numbers after a session ends.",
    "Expected: the 5-hour bar moves as soon as the session finishes.",
    "Steps: run a session to the end, then open the usage popover from the sidebar.",
    "Adds your app and macOS version",
  ]);
  assert.deepEqual(seen.repeat, [
    "Same here: pressing Stop on a worker doesn't stop it. It keeps writing files until it finishes on its own.",
    `Goes on #${417} as a comment`,
  ]);
  const note = "Usage, Stop and pinning\n\nThe usage popover keeps the old numbers after a session ends. Stop on a worker doesn't stop it. They'd like to pin a chat to the top.";
  assert.equal(seen.edit, note);
  assert.deepEqual(seen.failed, ["Couldn’t send it: HTTP 502: Bad gateway. Try again in a moment.", ["Try again", "Edit", "Dismiss"]]);
  // Written once each, never again on Send.
  assert.deepEqual(seen.wrote, ["Typo fixes take long\n\nA one-word fix took four minutes.", note]);
  assert.deepEqual(seen.opened, []);
  assert.deepEqual(seen.sent, ["Sent. Thank you", "You can follow them on GitHub."]);
  assert.deepEqual(seen.rows, [
    "Usage popover doesn't refresh when a session ends#482",
    `Stop doesn't stop a running workerAdded to #${417}`,
    "Pin a chat to the top of the sidebar#484",
  ]);
  assert.equal(seen.left, 1);
  assert.deepEqual(seen.answers, [
    { method: "answerFeedbackOffer", conversationId: (seen.answers as { conversationId: string }[])[0]?.conversationId, offerId: "second", sent: true },
    { method: "answerFeedbackOffer", conversationId: (seen.answers as { conversationId: string }[])[0]?.conversationId, offerId: "first", sent: false },
  ]);
  assert.equal(seen.dialogOpen, false);
});

// When only some reports were filed, the card lists them with Try again, which files the rest
// alone; the offer is answered once.
test("a feedback offer that sent only some files the rest on Try again", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1&done=1&partial=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.partial, [
    "Not all of it was sent",
    "One report couldn’t be sent.",
    ["Usage popover doesn't refresh when a session ends#482", `Stop doesn't stop a running workerAdded to #${417}`, "Pin a chat to the top of the sidebarNot sent"],
    ["Try again"],
  ]);
  assert.deepEqual(seen.sent, [
    "Sent. Thank you",
    "You can follow them on GitHub.",
    ["Usage popover doesn't refresh when a session ends#482", `Stop doesn't stop a running workerAdded to #${417}`, "Pin a chat to the top of the sidebar#482"],
    [],
  ]);
  assert.deepEqual(seen.filed, [3, 1]);
  assert.equal(seen.answers, 1);
});

// Signed out, the card signs in to GitHub under its reports (no forms open): its one-time code,
// Open GitHub, then it waits; Send is off, Edit hidden, Dismiss stays; once GitHub says yes it
// files on its own.
test("a feedback offer sent signed out signs in to GitHub, then files", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback-offer.html?drive=1&done=1&signedout=1", "feedback-offer-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.head, ["Send these to Brigadier’s developers?", "3 reports written from this chat"]);
  assert.deepEqual(seen.signIn, [
    ["Sign in to GitHub so Brigadier can post your reports", "WDJB-MJHTCopy codeOpen GitHub"],
    ["Send", "Dismiss"],
    true,
    3,
  ]);
  assert.deepEqual(seen.waiting, [
    ["Sign in to GitHub so Brigadier can post your reports", "WDJB-MJHTCopy codeOpen GitHub", "Waiting for GitHub…"],
    ["https://github.com/login/device"],
  ]);
  assert.deepEqual(seen.sent, [
    "Sent. Thank you",
    "You can follow them on GitHub.",
    ["Usage popover doesn't refresh when a session ends#482", `Stop doesn't stop a running workerAdded to #${417}`, "Pin a chat to the top of the sidebar#484"],
  ]);
  assert.deepEqual(seen.asked, ["startGitHubSignIn", "waitGitHubSignIn", "fileFeedback"]);
});
