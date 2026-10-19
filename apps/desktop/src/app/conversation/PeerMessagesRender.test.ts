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

type Turn = { state: string | null; label: string | null; shimmer: boolean; header: string | null; status: string | null };
type Row = { state: string | null; label: string | null; links: string[]; chevron: boolean };

test("Messages between sessions read in every state, on both sides", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "peer-messages.html?drive=gallery", "peer-result", 90000)) as {
    error?: string;
    liveRow: Turn;
    rows: Row[];
    groups: string[];
    replyInSender: { id: string | null; caption: string | null; inTurn: boolean }[];
    toAnswer: Landing & { ms: number; text: string | null };
    blocks: {
      id: string; caption: string; captionFont: number[]; label: string; inTurn: boolean; asInput: boolean;
      box: number[]; sameAsUser: boolean; more: boolean;
    }[];
    showMore: { clipped: number; opened: number };
    docsRows: Row[];
  };
  assert.equal(seen.error, undefined);
  // The newest turn waits for Docs inside the turn: one row, shimmering, under the live header.
  assert.deepEqual({ ...seen.liveRow, header: seen.liveRow.header?.replace(/\d.*$/, "") }, {
    state: "waiting", label: "Waiting for Docs to reply", shimmer: true, header: "Working for ", status: null,
  });
  // The sender's rows, one per exchange: what went where and the replies its waits took. A row
  // opens to nothing; each session it names is a link, but not one the app no longer has.
  assert.deepEqual(seen.rows, [
    { state: "waited", label: "Sent message to Docs, waited for its reply", links: ["Docs"], chevron: false },
    { state: "seen", label: "Sent message to Release notes", links: ["Release notes"], chevron: false },
    { state: "waited", label: "Sent messages to Docs and Release notes, waited for a reply from Docs", links: ["Docs", "Release notes", "Docs"], chevron: false },
    { state: "declined", label: "Didn’t send message to Docs", links: ["Docs"], chevron: false },
    { state: "refused", label: "Couldn’t send message to Billing", links: [], chevron: false },
    { state: "dropped", label: "Couldn’t send message to a session", links: [], chevron: false },
    { state: "waiting", label: "Waiting for Docs to reply", links: ["Docs"], chevron: false },
  ]);
  // A turn whose only work is its message folds no group of its own: the row stands alone.
  assert.deepEqual(seen.groups, ["Read files, messaged a session"]);
  // A reply a wait took isn't in the sender's thread: the turn's answer says it. A late reply, and
  // a message that opened a turn, stand as the turn's input, as the user's own would.
  assert.deepEqual(seen.replyInSender, [
    { id: "q1", caption: "Sent by Docs from another session", inTurn: false },
    { id: "m3b-reply", caption: "Sent by Release notes from another session", inTurn: false },
  ]);
  // The late reply's caption opens Release notes at the answer that reply carried.
  assert.equal(seen.toAnswer.found, true);
  assert.equal(seen.toAnswer.revealed, true);
  assert.equal(seen.toAnswer.inView, true);
  assert.equal(seen.toAnswer.inTurn, true);
  assert.equal(seen.toAnswer.selection, "peer-notes");
  assert.equal(seen.toAnswer.text, "Not yet: I'll add it with the 2.4 notes.");

  // The receiver's messages: the user's own bubble, a caption naming the sender above it.
  assert.deepEqual(seen.blocks.map(({ id, caption, inTurn, asInput }) => ({ id, caption, inTurn, asInput })), [
    { id: "m1", caption: "Sent by API schema from another session", inTurn: false, asInput: true },
    { id: "m3", caption: "Sent by API schema from another session", inTurn: true, asInput: false },
    { id: "s1", caption: "Sent by Style guide from another session", inTurn: false, asInput: true },
    { id: "m7", caption: "Sent by API schema from another session", inTurn: false, asInput: true },
  ]);
  for (const block of seen.blocks) {
    // No outline, the corner radius and padding of the user's own bubble, its fill too.
    assert.deepEqual(block.box, [0, 22, 10, 16], block.id);
    assert.equal(block.sameAsUser, true, block.id);
    // The caption's size and line height.
    assert.deepEqual(block.captionFont, [13, 19.5], block.id);
    assert.equal(block.label, block.caption.includes("Style guide") ? "Message from Style guide:" : "Message from API schema:");
  }
  // A long message folds to a few lines, and Show more opens all of it.
  assert.deepEqual(seen.blocks.map((block) => block.more), [true, false, false, false]);
  assert.ok(seen.showMore.opened > seen.showMore.clipped, `${seen.showMore.opened} > ${seen.showMore.clipped}`);
  // Docs' own question, waited for and answered in one turn; its answers to API schema add no rows.
  assert.deepEqual(seen.docsRows, [{ state: "waited", label: "Sent message to API schema, waited for its reply", links: ["API schema"], chevron: false }]);
});

test("The sender's newest turn says what it does now: sending, waiting for the user's OK, waiting for the reply, or resting on it", { timeout: 120000 }, async (t) => {
  const turn = async (live: string) => {
    const seen = JSON.parse(await renderFixtureLive(t, `peer-messages.html?drive=live${live}`, "peer-result", 90000)) as Turn & { error?: string };
    assert.equal(seen.error, undefined);
    return { ...seen, header: seen.header?.replace(/\d.*$/, "") ?? null };
  };
  // The row says it sends: no live line under it.
  assert.deepEqual(await turn("&live=sending"), { state: "sending", label: "Sending message to Docs", shimmer: true, header: "Working for ", status: null });
  assert.deepEqual(await turn("&live=ok"), { state: "waitingForOk", label: "Waiting for your OK to message Docs", shimmer: false, header: "Working for ", status: "Thinking" });
  // Its wait ran out before Docs replied: the turn rests on the reply, which comes as a message.
  assert.deepEqual(await turn("&live=rested"), {
    state: "waited", label: "Sent message to Docs, waited; no reply yet", shimmer: false, header: "Waiting for Docs to reply · ", status: "Waiting for Docs to reply",
  });
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
  // It lands once its folds are still, not when the wait for them runs out (4 s from the jump):
  // timed from its row showing (fold animations and a few still frames, under a second even
  // with the CPU slowed), so how fast the machine reads and draws the session doesn't count.
  assert.ok(seen.toReceiver.ms < 2000, `landed ${seen.toReceiver.ms} ms after its row showed`);
  assert.deepEqual(seen.afterOpen, none);
  // Back from the received message's caption to its sent row, deep in folded work 32 turns back.
  assert.equal(seen.hadCaption, true);
  assertLanded(seen.toSender, "peer-api");
  // Reopened, the sender holds its newest page only: the jump read older ones to reach the row.
  assert.equal(seen.toSender.pagesRead, true);
  // Left on another tab, the sender shows its chat, where the row is.
  assert.equal(seen.toSender.chat, true);
  // A long group's list fades its edges as it scrolls, an animation that never ends: no fold.
  assert.ok(seen.toSender.ms < 2000, `landed ${seen.toSender.ms} ms after its row showed`);
  assert.equal(seen.toSender.workOpen, true);
  assert.equal(seen.toSender.group, "open");
  assert.deepEqual(seen.afterBack, none);
  // A message to the open session leaves no dot; one to another session does, until it's opened.
  assert.deepEqual(seen.afterArrival, { ...none, "peer-docs": true });
  assert.deepEqual(seen.afterReopen, none);
});

// #336: on a slow machine the replies' renderer can load just after a jump lands.
test("A jump stays centred on its message when the replies' renderer loads after it lands", { timeout: 150000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "peer-messages.html?history=long&drive=jump&renderer=late", "peer-result", 120000, { latencyMs: 300 })) as {
    error?: string; toReceiver: Landing; toSender: Landing;
  };
  assert.equal(seen.error, undefined);
  assertLanded(seen.toReceiver, "peer-docs");
  assertLanded(seen.toSender, "peer-api");
});

test("a second send mid-wait has its own Sending row until its wait arrives or the turn ends", { timeout: 120000 }, async (t) => {
  const rows = async (state: string) => JSON.parse(await renderFixtureLive(t,
    `peer-messages.html?drive=live&live=second${state}`, "peer-result", 90000)) as { state: string; label: string }[];
  const sending = await rows("");
  assert.equal(sending.length, 2);
  assert.deepEqual(sending[1], { state: "sending", label: "Sending message to Release notes" });
  const waiting = await rows("-wait");
  assert.equal(waiting.length, 1);
  assert.equal(waiting[0]?.state, "waiting");
  const ended = await rows("-done");
  assert.equal(ended.length, 1);
  assert.match(ended[0]?.label ?? "", /^Sent messages to Docs and Release notes, waited/);
});
