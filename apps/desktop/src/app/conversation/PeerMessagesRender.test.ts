import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Landing = {
  found: boolean; revealed: boolean; offCentre: number | null; inView: boolean; clamped: boolean;
  group: string | null; workOpen: boolean; inTurn: boolean; selection: string | null;
};
type Dots = Record<"peer-api" | "peer-docs" | "peer-notes" | "peer-style", boolean>;

/** A jump that landed on its message: found, flashed, centred in the thread and inside its turn. */
function assertLanded(landing: Landing, selection: string): void {
  assert.equal(landing.found, true);
  assert.equal(landing.revealed, true);
  assert.ok(landing.offCentre !== null && Math.abs(landing.offCentre) <= 2, `centred, ${landing.offCentre}px off`);
  assert.equal(landing.inView, true);
  assert.equal(landing.clamped, false);
  assert.equal(landing.inTurn, true);
  assert.equal(landing.selection, selection);
}

test("Messages between sessions read in every state, on both sides", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "peer-messages.html?drive=gallery", "peer-result", 90000)) as {
    error?: string;
    rows: { state: string; label: string; link: boolean; line: string | null }[];
    lineShown: boolean;
    whole: string | null;
    groups: string[];
    replyInSender: { caption: string; inTurn: boolean }[];
    openReply: Landing;
    blocks: { id: string; caption: string; label: string; inTurn: boolean; asInput: boolean; outline: string; fill: string; more: boolean }[];
    showMore: { clipped: number; opened: number };
    waiting: { header: string; status: string };
    docsRows: { state: string; label: string }[];
  };
  assert.equal(seen.error, undefined);
  // The sender's rows: the session (and its project when it's another), the state under the text,
  // and a link to the receiver only when there is one to open.
  assert.deepEqual(seen.rows, [
    { state: "replied", label: "Messaged Docs in Handbook", link: true, line: "Replied · Open reply" },
    { state: "seen", label: "Messaged Release notes", link: true, line: "Seen" },
    { state: "delivered", label: "Messaged Docs in Handbook", link: true, line: "Delivered" },
    { state: "declined", label: "Didn’t message Docs in Handbook", link: true, line: "You didn’t send it" },
    { state: "refused", label: "Couldn’t message Billing", link: false, line: "Billing is archived, so it can't take messages." },
    { state: "dropped", label: "Couldn’t message a session", link: false, line: "Couldn’t deliver: the session was deleted" },
    { state: "waitingForOk", label: "Waiting for your OK to message Docs in Handbook", link: true, line: "Waiting for your OK" },
    { state: "pending", label: "Messaging Release notes", link: true, line: null },
  ]);
  // Opened inside its group's scrolling list, a row brings its state line into the list's view.
  assert.equal(seen.lineShown, true);
  // A message too long for its record shows its whole text, read from the blob store.
  assert.match(seen.whole ?? "", /The rest of the long note: the migration runs before the deploy\./);
  assert.deepEqual(seen.groups, ["Read files and messaged a session", "Messaging Release notes"]);
  // In the sender, the reply sits inside the turn that asked; a message that opened a turn stands as its input.
  assert.deepEqual(seen.replyInSender, [
    { caption: "Reply from Docs", inTurn: true },
    { caption: "From Docs · Handbook", inTurn: false },
  ]);
  // "Open reply" lands on the reply in the same session.
  assertLanded(seen.openReply, "peer-api");

  // The receiver's blocks: an outline, no fill, a caption naming the sender, read out as a message from it.
  assert.deepEqual(seen.blocks.map(({ id, caption, inTurn, asInput }) => ({ id, caption, inTurn, asInput })), [
    { id: "m1", caption: "From API schema · Website", inTurn: false, asInput: true },
    { id: "m3", caption: "From API schema · Website", inTurn: true, asInput: false },
    { id: "q1-reply", caption: "Reply from API schema · to your earlier message", inTurn: false, asInput: true },
    { id: "s1", caption: "From Style guide", inTurn: false, asInput: true },
  ]);
  for (const block of seen.blocks) {
    assert.equal(Number.parseFloat(block.outline), 1, block.id);
    // A fill with no alpha: the block is clear, its outline alone sets it apart.
    assert.match(block.fill, /,\s*0\)$/, block.id);
    assert.equal(block.label, block.caption.includes("Style guide") ? "Message from Style guide:" : "Message from API schema:");
  }
  // A long message folds to a few lines, and Show more opens all of it.
  assert.deepEqual(seen.blocks.map((block) => block.more), [true, false, false, false]);
  assert.ok(seen.showMore.opened > seen.showMore.clipped, `${seen.showMore.opened} > ${seen.showMore.clipped}`);
  // A turn resting on a reply says whom it waits for, in its header and its live line.
  assert.match(seen.waiting.header, /^Waiting for API schema · /);
  assert.equal(seen.waiting.status, "Waiting for API schema");
  assert.deepEqual(seen.docsRows, [
    { state: "seen", label: "Messaged API schema in Website" },
    { state: "replied", label: "Messaged API schema in Website" },
    { state: "delivered", label: "Messaged API schema in Website" },
  ]);
});

test("A jump opens the other session at the exact message, both ways, and the sidebar marks a message not yet seen", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "peer-messages.html?history=long&drive=jump", "peer-result", 90000)) as {
    error?: string;
    before: Dots; toReceiver: Landing & { pagesRead: boolean; loadedBefore: number; ms: number }; afterOpen: Dots; hadCaption: boolean;
    toSender: Landing & { pagesRead: boolean; ms: number; chat: boolean }; afterBack: Dots; afterArrival: Dots; afterReopen: Dots;
  };
  assert.equal(seen.error, undefined);
  const none: Dots = { "peer-api": false, "peer-docs": false, "peer-notes": false, "peer-style": false };
  // Docs got a message while it wasn't open.
  assert.deepEqual(seen.before, { ...none, "peer-docs": true });
  // From the sent row to the message, steered into a turn on a page not yet read.
  assert.equal(seen.toReceiver.loadedBefore, 0);
  assert.equal(seen.toReceiver.pagesRead, true);
  assertLanded(seen.toReceiver, "peer-docs");
  // It lands once its folds are still, not when the wait for them runs out (4 s).
  assert.ok(seen.toReceiver.ms < 3000, `landed in ${seen.toReceiver.ms} ms`);
  assert.deepEqual(seen.afterOpen, none);
  // Back from the received message's caption to its sent row, deep in folded work 32 turns back.
  assert.equal(seen.hadCaption, true);
  assertLanded(seen.toSender, "peer-api");
  // Reopened, the sender holds its newest page only: the jump read older ones to reach the row.
  assert.equal(seen.toSender.pagesRead, true);
  // Left on another tab, the sender shows its chat, where the row is.
  assert.equal(seen.toSender.chat, true);
  // A long group's list fades its edges as it scrolls, an animation that never ends: no fold.
  assert.ok(seen.toSender.ms < 3000, `landed in ${seen.toSender.ms} ms`);
  assert.equal(seen.toSender.workOpen, true);
  assert.equal(seen.toSender.group, "open");
  assert.deepEqual(seen.afterBack, none);
  // A message to the open session leaves no dot; one to another session does, until it's opened.
  assert.deepEqual(seen.afterArrival, { ...none, "peer-docs": true });
  assert.deepEqual(seen.afterReopen, none);
});
