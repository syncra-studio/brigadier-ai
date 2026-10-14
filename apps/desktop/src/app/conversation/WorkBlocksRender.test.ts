import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  header: string | null; newest: string | null; newestShimmers: boolean; fold: string; groupRow: string | null;
  groupLine: string | null; groupTone: string | null; shimmer: boolean; status: string | null; thinkingLive: number;
  commentary: string | null; answer: string | null; answerInWork: boolean; statusAfterAnswer: boolean; statusInWork: boolean;
};

async function drive<T>(t: test.TestContext, query: string): Promise<T> {
  const seen = JSON.parse(await renderFixtureLive(t, `work-blocks.html?${query}`, "work-blocks-result", 90000)) as T & { error?: string };
  assert.equal(seen.error, undefined);
  return seen;
}

test("a turn has one live line, folds the moment its final answer starts, and keeps that time through the end and a reload", { timeout: 120000 }, async (t) => {
  const seen = await drive<Record<"thinking" | "thought" | "question" | "running" | "commentary" | "answering" | "answeringLater" | "stored" | "waiting" | "done" | "reloaded", Seen> & {
    foldMs: number; actions: string[];
  }>(t, "drive=turn");
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
  // A line between steps shimmers on one line while written, under the header, the group above summed up.
  assert.deepEqual(
    [seen.commentary.newest, seen.commentary.newestShimmers, seen.commentary.groupRow, seen.commentary.status, seen.commentary.header?.startsWith("Working for")],
    ["I’ll fix the rounding next.", true, "Read files, ran commands", null, true],
  );
  // The final answer starts: the work folds within the 150ms close and a frame or two.
  assert.ok(seen.foldMs >= 0 && seen.foldMs <= 250, `folded in ${seen.foldMs} ms`);
  const header = seen.answering.header;
  assert.match(header ?? "", /^Worked for \d+s$/);
  for (const state of [seen.answering, seen.answeringLater, seen.stored, seen.waiting, seen.done, seen.reloaded]) {
    // Folded, the answer outside the work, the time stopped where the answer started.
    assert.deepEqual([state.fold, state.answerInWork, state.header], ["gone", false, header]);
    assert.notEqual(state.answer, null);
  }
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
  const seen = await drive<{ before: Seen; after: Seen; bubble: string | null }>(t, "drive=steer");
  assert.deepEqual([seen.before.newest, seen.before.newestShimmers], ["Running sleep 30", true]);
  assert.deepEqual([seen.after.groupRow, seen.after.shimmer], ["Ran a command", false]);
  // The live line moves under the bubble, though the command still runs.
  assert.deepEqual([seen.after.status, seen.after.statusInWork], ["Thinking", true]);
  assert.match(seen.bubble ?? "", /^Use banker's rounding instead/);
});

test("a queued message's ⋯ opens it in a side chat in a session, and turns queueing off", { timeout: 120000 }, async (t) => {
  const chat = await drive<{ items: string[]; followUp: string; saved: string }>(t, "drive=queue");
  assert.deepEqual(chat, { items: ["Edit message", "Turn off queueing"], followUp: "steer", saved: "steer" });
  const session = await drive<{ items: string[] }>(t, "drive=queue&kind=session");
  assert.deepEqual(session.items, ["Edit message", "Open in side chat", "Turn off queueing"]);
});
