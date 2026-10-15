import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  header: string | null; newest: string | null; newestShimmers: boolean; fold: string; groupRow: string | null;
  groupLine: string | null; groupTone: string | null; shimmer: boolean; status: string | null; thinkingLive: number;
  commentary: string | null; commentaryShimmers: boolean; answer: string | null; answerInWork: boolean; statusAfterAnswer: boolean; statusInWork: boolean;
};

async function drive<T>(t: test.TestContext, query: string): Promise<T> {
  const seen = JSON.parse(await renderFixtureLive(t, `work-blocks.html?${query}`, "work-blocks-result", 90000)) as T & { error?: string };
  assert.equal(seen.error, undefined);
  return seen;
}

test("a turn has one live line, folds the moment its final answer starts, and keeps that time through the end and a reload", { timeout: 120000 }, async (t) => {
  const seen = await drive<Record<"thinking" | "thought" | "question" | "running" | "justEnded" | "ended" | "commentary" | "answering" | "answeringLater" | "answeringAsked" | "stored" | "waiting" | "done" | "reloaded", Seen> & {
    foldFirst: string; foldMs: number; actions: string[]; firstHeader: string | null; commentaryAtOnce: string | null;
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
  // A step running says itself, shimmering: still one line.
  assert.deepEqual([seen.running.groupRow, seen.running.shimmer, seen.running.groupLine, seen.running.status], ["Running sleep 4", true, null, null]);
  // A command that just ended still names its group for a second, then the group says "Thinking".
  assert.deepEqual([seen.justEnded.groupRow, seen.justEnded.shimmer, seen.justEnded.groupLine], ["Running sleep 4", true, null]);
  assert.deepEqual([seen.ended.groupLine, seen.ended.groupTone], ["Thinking", "busy"]);
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
  assert.deepEqual(seen.actions, ["Copy", "Read aloud", "Rate response", "Try again", "Fork chat from here"]);
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
