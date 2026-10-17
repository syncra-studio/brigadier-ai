import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  header: string | null; newest: string | null; newestShimmers: boolean; fold: string; groupRow: string | null; groupIcons: number; groupAnimations: string[];
  groupLine: string | null; groupTone: string | null; groupGlyph: string | null; groupQuiet: boolean; shimmer: boolean; status: string | null; thinkingLive: number;
  commentary: string | null; commentaryShimmers: boolean; answer: string | null; answerInWork: boolean; statusAfterAnswer: boolean; statusInWork: boolean;
};

async function drive<T>(t: test.TestContext, query: string): Promise<T> {
  const seen = JSON.parse(await renderFixtureLive(t, `work-blocks.html?${query}`, "work-blocks-result", 90000)) as T & { error?: string };
  assert.equal(seen.error, undefined);
  return seen;
}

test("a turn whose answer starts while the window is hidden folds its work at once", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ shown: boolean; gone: boolean }>(t, "drive=hidden");
  assert.deepEqual(seen, { shown: true, gone: true });
});

test("a turn has one live line, folds the moment its final answer starts, and keeps that time through the end and a reload", { timeout: 120000 }, async (t) => {
  const seen = await drive<Record<"thinking" | "thought" | "question" | "approval" | "running" | "justEnded" | "ended" | "commentary" | "answering" | "answeringLater" | "answeringAsked" | "stored" | "waiting" | "done" | "reloaded", Seen> & {
    foldFirst: string; foldMs: number; actions: string[]; firstHeader: string | null; commentaryAtOnce: string | null;
    announcedAnswering: string | null; announcedDone: string | null; time: { text: string | null; opacity: string | null }; html: string | null;
  }>(t, "drive=turn");
  // The header is there from the turn's first frame.
  assert.match(seen.firstHeader ?? "", /^Working/);
  // Nothing runs: the newest group's own row says "Thinking"; no line of its own under it.
  assert.deepEqual(
    [seen.thinking.groupLine, seen.thinking.groupTone, seen.thinking.shimmer, seen.thinking.status, seen.thinking.thinkingLive],
    ["Thinking", "busy", true, null, 0],
  );
  // The live thought's heading, in the same row.
  assert.deepEqual([seen.thought.groupLine, seen.thought.status, seen.thought.thinkingLive], ["Checking the failing test", null, 0]);
  // What waits on the user wins over the thought.
  assert.deepEqual([seen.question.groupLine, seen.question.groupTone, seen.question.status, seen.question.thinkingLive], ["Waiting for your answer", "needsYou", null, 0]);
  // A question's wait has a checklist before its words; "Thinking" has no glyph.
  assert.deepEqual([seen.question.groupGlyph, seen.question.groupQuiet, seen.thinking.groupGlyph], ["tasks", false, null]);
  // A call waiting for the user's OK: "Awaiting approval", its sweep over fainter words, no glyph.
  assert.deepEqual(
    [seen.approval.groupLine, seen.approval.groupTone, seen.approval.groupQuiet, seen.approval.groupGlyph, seen.approval.status],
    ["Awaiting approval", "needsYou", true, null, null],
  );
  // A step running says itself, shimmering: still one line.
  assert.deepEqual([seen.running.groupRow, seen.running.shimmer, seen.running.groupLine, seen.running.status], ["Running sleep 4", true, null, null]);
  // A command that just ended still names its group for a second, then the group says "Thinking".
  assert.deepEqual([seen.justEnded.groupRow, seen.justEnded.shimmer, seen.justEnded.groupLine], ["Running sleep 4", true, null]);
  assert.deepEqual([seen.ended.groupLine, seen.ended.groupTone], ["Thinking", "busy"]);
  // A step's row has the step's icon; "Thinking" is no step's, so only the chevron is left.
  assert.deepEqual([seen.running.groupIcons, seen.ended.groupIcons], [2, 1]);
  // A live label's words show at once and sweep: no fade-in, which would start them invisible and
  // replace the sweep.
  assert.deepEqual([seen.running.groupAnimations, seen.ended.groupAnimations], [["shimmer"], ["shimmer"]]);
  // A line between steps streams as prose under the header, never shimmering, each piece shown
  // the frame it arrives (never paced like the answer); the group above sums up.
  assert.equal(seen.commentaryAtOnce, "I’ll fix the rounding next. Then run the tests again.");
  assert.deepEqual(
    [seen.commentary.commentary, seen.commentary.commentaryShimmers, seen.commentary.groupRow, seen.commentary.status, seen.commentary.header?.startsWith("Working for")],
    ["I’ll fix the rounding next. Then run the tests again.", false, "Read files, ran commands", null, true],
  );
  // The final answer starts: the frame that shows it closes the work, gone within the 150ms
  // close and a frame or two.
  assert.equal(seen.foldFirst, "closed");
  assert.ok(seen.foldMs >= 0 && seen.foldMs <= 250, `folded in ${seen.foldMs} ms`);
  const header = seen.answering.header;
  assert.match(header ?? "", /^Worked for \d+s$/);
  for (const state of [seen.answering, seen.answeringLater, seen.stored, seen.waiting, seen.done, seen.reloaded]) {
    // Folded, the answer outside the work, the time stopped where the answer started.
    assert.deepEqual([state.fold, state.answerInWork, state.header], ["gone", false, header]);
    assert.notEqual(state.answer, null);
  }
  for (const state of [seen.answering, seen.answeringLater, seen.stored]) {
    assert.deepEqual(
      [state.status, state.statusAfterAnswer, state.statusInWork],
      ["Waiting for Billing to reply", true, false],
    );
  }
  assert.equal(seen.answeringAsked.status, "Waiting for your answer");
  assert.equal(seen.stored.answer, "Fixed the rounding: round now rounds half-up, and the tests pass.");
  // Answered but waiting for another session's reply: that line stays, under the answer, outside the work.
  assert.deepEqual(
    [seen.waiting.status, seen.waiting.statusAfterAnswer, seen.waiting.statusInWork],
    ["Waiting for Billing to reply", true, false],
  );
  assert.equal(seen.done.status, null);
  assert.deepEqual(seen.actions, ["Copy response", "Read aloud", "Feedback", "Try again", "Fork chat from here"]);
  // The latest answer's time shows without a hover.
  assert.equal(seen.time.opacity, "1");
  assert.match(seen.time.text ?? "", /\d:\d\d/);
  // A screen reader hears the answer start, then end with its words.
  assert.equal(seen.announcedAnswering, "Response started");
  assert.equal(seen.announcedDone, "Response complete: Fixed the rounding: `round` now rounds half-up, and the tests pass.");
  // Copy's rich text is the answer as shown, with no classes or controls.
  assert.equal(seen.html, "<div><p>Fixed the rounding: <code>round</code> now rounds half-up, and the tests pass.</p></div>");
});

test("a turn that ends in a plan keeps its work open, and the user's choice outlives a remount", { timeout: 120000 }, async (t) => {
  const seen = await drive<Record<"ended" | "closed" | "remounted", Seen> & { opened: Record<string, unknown> }>(t, "drive=plan");
  assert.match(seen.ended.header ?? "", /^Worked for/);
  // Closed, its fold is gone; remounted, it stays closed where the plan would open it.
  assert.deepEqual([seen.ended.fold, seen.closed.fold, seen.remounted.fold], ["open", "gone", "gone"]);
  // A chat has no tabs: its plan doesn't offer to open.
  assert.deepEqual(seen.opened, { canOpen: false, active: "chat", kinds: [], shown: false, tab: null, rightPanel: false });
});

test("a session's plan opens full screen in its own tab, not the right panel", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ opened: Record<string, unknown> }>(t, "drive=plan&kind=session");
  assert.deepEqual(seen.opened, { canOpen: true, active: "plan:plan:plan", kinds: ["plan"], shown: true, tab: "Round half-up", rightPanel: false });
});

test("a turn's denied calls are counted in its header, which opens the work to the first", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ before: { denied: string | null; fold: string }; fold: string; row: string | null }>(t, "drive=denied");
  assert.deepEqual(seen.before, { denied: "1 denied action", fold: "gone" });
  assert.equal(seen.fold, "open");
  assert.match(seen.row ?? "", /git push origin main/);
});

test("a long message is clipped at the end of a code block crossing its 20-line cut, Show more with a chevron", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ clipEnd: number | null; blockEnd: number | null; clipLines: number | null; more: boolean; chevron: boolean; sides: string[] | null }>(t, "drive=clip");
  // The block crossing line 20 shows whole: the clip ends where it does, past 20 lines.
  assert.ok(seen.blockEnd !== null && seen.clipEnd !== null && Math.abs(seen.clipEnd - seen.blockEnd) <= 1, `clip ${seen.clipEnd}, block ${seen.blockEnd}`);
  assert.ok((seen.clipLines ?? 0) > 20, `clipped at ${seen.clipLines} lines`);
  assert.deepEqual([seen.more, seen.chevron], [true, true]);
  // It cuts below only: a list's "10." hanging left of its text keeps its first digit.
  assert.deepEqual(seen.sides, ["visible", "clip"]);
});

test("a message edited in place is a composer the column's width, its Cancel and Send pills", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ latest: string | null; edit: boolean; fill: boolean | null; radius: number | null; wide: number | null; cancel: { round: boolean; border: number } | null; send: { round: boolean; border: number } | null }>(t, "drive=edit");
  // A screen reader's "Latest response" took the focus to the newest answer.
  assert.equal(seen.latest, "assistant");
  assert.equal(seen.edit, true);
  assert.equal(seen.fill, true);
  assert.ok((seen.radius ?? 0) >= 18, `corners ${seen.radius}`);
  assert.ok((seen.wide ?? 0) > 0.9, `width ${seen.wide}`);
  assert.deepEqual([seen.cancel, seen.send], [{ round: true, border: 1 }, { round: true, border: 0 }]);
});

test("messages that fail to load say so above the composer, and Retry loads them", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ failed: { line: string | null; role: string | null; rows: number }; after: { line: string | null; rows: number } }>(t, "drive=loadfail");
  assert.deepEqual(seen.failed, { line: "Couldn’t load messagesRetry", role: "alert", rows: 0 });
  assert.equal(seen.after.line, null);
  assert.ok(seen.after.rows > 0, `${seen.after.rows} messages after Retry`);
});

test("text selected in an answer gets a pill: Add to chat quotes it into the composer and keeps it marked; Ask in side chat opens one with it", { timeout: 120000 }, async (t) => {
  const seen = await drive<{
    pills: number; placed: { bottomGap: number; centre: number } | null; buttons: (string | null)[]; composer: string | null; focusedOwn: boolean; marked: number;
    afterAdd: { pill: boolean; collapsed: boolean | null }; sideChat: string[]; draft: string | null;
  }>(t, "drive=select&kind=session");
  // 8px above the selection, centred on it.
  assert.ok(seen.placed && Math.abs(seen.placed.bottomGap - 8) <= 1 && Math.abs(seen.placed.centre) <= 1, JSON.stringify(seen.placed));
  assert.deepEqual(seen.buttons, ["Add to chat", "Ask in side chat"]);
  assert.match(seen.composer ?? "", /> Rounding now uses half-up everywhere\./);
  assert.equal(seen.focusedOwn, true);
  assert.equal(seen.marked, 1);
  assert.deepEqual(seen.afterAdd, { pill: false, collapsed: true });
  // One side chat of this session (its tab asks for it again as it opens).
  assert.equal(seen.pills, 1);
  assert.deepEqual([...new Set(seen.sideChat)], ["openSideChat work-blocks"]);
  assert.equal(seen.draft, "> Rounding now uses half-up everywhere.\n\n");
});

test("a message steered in before the turn did anything sits under the first, above its header", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ bubble: string | null; above: boolean; header: string | null; status: string | null; stillAbove: boolean }>(t, "drive=early");
  assert.match(seen.bubble ?? "", /Use banker's rounding instead/);
  assert.equal(seen.above, true);
  assert.match(seen.header ?? "", /^Working/);
  assert.equal(seen.status, "Thinking");
  // Rows coming later go under the header, the bubble stays above it.
  assert.equal(seen.stillAbove, true);
});

test("a message steered in while a command runs sums the work above it up at once", { timeout: 120000 }, async (t) => {
  const seen = await drive<{
    before: Seen; after: Seen; bubble: string | null; deciding: { note: string | null; shimmer: string | null };
    steerHeld: string | null; steerRead: string | null;
  }>(t, "drive=steer");
  assert.deepEqual([seen.before.newest, seen.before.newestShimmers], ["Running sleep 30", true]);
  assert.deepEqual([seen.after.groupRow, seen.after.shimmer], ["Ran a command", false]);
  // The live line moves under the bubble, though the command still runs.
  assert.deepEqual([seen.after.status, seen.after.statusInWork], ["Thinking", true]);
  assert.match(seen.bubble ?? "", /^Use banker's rounding instead/);
  // Behind the thread's command, a follow-up and a steered message say when they are read, not "Deciding".
  assert.deepEqual(seen.deciding, { note: "Read when the current command ends · running 2m", shimmer: null });
  assert.equal(seen.steerHeld, "Read when the current command ends · running 2m");
  assert.equal(seen.steerRead, null);
});

test("a queued message's ⋯ opens it in a side chat in a session, and turns queueing off", { timeout: 120000 }, async (t) => {
  const chat = await drive<{ items: string[]; followUp: string; saved: string }>(t, "drive=queue");
  assert.deepEqual(chat, { items: ["Edit message", "Turn off queueing"], followUp: "steer", saved: "steer" });
  const session = await drive<{ items: string[] }>(t, "drive=queue&kind=session");
  assert.deepEqual(session.items, ["Edit message", "Open in side chat", "Turn off queueing"]);
});

test("a check gone to the background keeps its request working, says what runs and for how long, and its row stops it", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ running: Seen & { said: boolean; row: string | null; elapsed: string | null }; calls: string[]; done: Seen & { row: string | null } }>(
    t, "drive=background&kind=session",
  );
  // The thread's word on the run is a line in the work; the request still works, its live line the run's.
  assert.match(seen.running.header ?? "", /^Working for/);
  assert.equal(seen.running.said, true);
  assert.deepEqual([seen.running.status, seen.running.statusInWork], ["Running cargo test --workspace · 6m", true]);
  // The context card's row: the command, its time, Stop.
  assert.match(seen.running.row ?? "", /cargo test --workspace.*6m.*Stop/);
  assert.equal(seen.running.elapsed, "6m");
  assert.deepEqual(seen.calls, ["stopBackgroundRun run-1"]);
  // The result's answer settles it: no live line, no row, the answer outside the folded work.
  assert.deepEqual([seen.done.status, seen.done.row, seen.done.answer], [null, null, "The full Rust tests pass: 812 tests, no failures."]);
  assert.match(seen.done.header ?? "", /^Worked for/);
});

test("the header shows the work and the wait for the user apart: Worked 15s · waited for you 15s", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ asking: Seen; done: Seen; noWait: Seen }>(t, "drive=timer");
  // Waiting on its card: the work so far, and the wait beside it, ticking.
  assert.match(seen.asking.header ?? "", /^Working for 10s · waiting for you \d+s$/);
  assert.equal(seen.done.header, "Worked 15s · waited for you 15s");
  assert.equal(seen.noWait.header, "Worked for 15s");
});

test("a finished answer's file chip holds still while a later turn works: never drawn again as plain code", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ same: boolean; connected: boolean; events: string[]; color: string | null }>(t, "drive=filelink&kind=session");
  // Each event of the later turn moves the conversation's time, which reads the checkout again.
  assert.deepEqual(seen.events, []);
  assert.equal(seen.same, true);
  assert.equal(seen.connected, true);
  assert.ok(seen.color, "the chip is there, in link blue");
});

test("a follow-up that joined a turn as it ended leaves the first answer in view, above its bubble", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ first: boolean; second: boolean; firstAboveBubble: boolean; header: string | null }>(t, "drive=joined&kind=session");
  assert.equal(seen.first, true, JSON.stringify(seen));
  assert.equal(seen.second, true, JSON.stringify(seen));
  assert.equal(seen.firstAboveBubble, true, JSON.stringify(seen));
});

test("a plan approved: its request's new steps show under the plan as they come", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ under: boolean; new: boolean }>(t, "drive=implement&kind=session");
  assert.equal(seen.new, true, JSON.stringify(seen));
  assert.equal(seen.under, true, JSON.stringify(seen));
});
