import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  header: string | null; newest: string | null; newestShimmers: boolean; fold: string; groupRow: string | null; groupIcons: number; groupAnimations: string[];
  groupLine: string | null; groupTone: string | null; groupGlyph: string | null; groupQuiet: boolean; shimmer: boolean; status: string | null; thinkingLive: number;
  commentary: string | null; commentaryShimmers: boolean; answer: string | null; answerInWork: boolean; statusAfterAnswer: boolean; statusInWork: boolean;
};

async function drive<T>(t: test.TestContext, query: string, options?: Parameters<typeof renderFixtureLive>[4]): Promise<T> {
  const seen = JSON.parse(await renderFixtureLive(t, `work-blocks.html?${query}`, "work-blocks-result", 90000, options)) as T & { error?: string };
  assert.equal(seen.error, undefined);
  return seen;
}

test("a turn whose answer starts while the window is hidden folds its work at once", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ shown: boolean; gone: boolean }>(t, "drive=hidden");
  assert.deepEqual(seen, { shown: true, gone: true });
});

test("a turn has one live line, folds the moment its final answer starts, and keeps that time through the end and a reload", { timeout: 120000 }, async (t) => {
  const seen = await drive<Record<"thinking" | "thought" | "question" | "approval" | "running" | "justEnded" | "ended" | "commentary" | "answering" | "answeringLater" | "answeringAsked" | "stored" | "done" | "reloaded", Seen> & {
    foldFirst: string; foldMs: number; foldLate: number; actions: string[]; firstHeader: string | null; commentaryAtOnce: string | null;
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
  assert.ok(seen.foldMs >= 0 && seen.foldLate <= 2, `folded in ${seen.foldMs} ms, ${seen.foldLate} frames past its close`);
  const header = seen.answering.header;
  assert.match(header ?? "", /^Worked for \d+s$/);
  for (const state of [seen.answering, seen.answeringLater, seen.stored, seen.done, seen.reloaded]) {
    // Folded, the answer outside the work, the time stopped where the answer started.
    assert.deepEqual([state.fold, state.answerInWork, state.header], ["gone", false, header]);
    assert.notEqual(state.answer, null);
  }
  assert.equal(seen.answeringAsked.status, "Waiting for your answer");
  assert.equal(seen.stored.answer, "Fixed the rounding: round now rounds half-up, and the tests pass.");
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
  assert.equal(seen.composer, "Rounding now uses half-up everywhere.");
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

test("a message steered in while a command runs leaves the command's row saying it runs, and sums the work above it up once it ends (issue 445)", { timeout: 120000 }, async (t) => {
  const seen = await drive<{
    before: Seen; after: Seen; stillRuns: string | null; ended: string | null; bubble: string | null;
    steerHeld: string | null; steerRead: string | null;
  }>(t, "drive=steer");
  assert.deepEqual([seen.before.newest, seen.before.newestShimmers], ["Running sleep 30", true]);
  // #445: never "Ran a command" while it still runs.
  assert.match(seen.stillRuns ?? "", /^Running sleep 30/);
  assert.equal(seen.ended, "Ran a command");
  // The live line moves under the bubble, though the command still runs.
  assert.deepEqual([seen.after.status, seen.after.statusInWork], ["Thinking", true]);
  assert.match(seen.bubble ?? "", /^Use banker's rounding instead/);
  assert.equal(seen.steerHeld, "Read when the current command ends · running 2m");
  assert.equal(seen.steerRead, null);
});

test("a queued message's ⋯ opens it in a side chat in a session, and turns queueing off", { timeout: 120000 }, async (t) => {
  const chat = await drive<{ items: string[]; followUp: string; saved: string }>(t, "drive=queue");
  assert.deepEqual(chat, { items: ["Edit message", "Turn off queueing"], followUp: "steer", saved: "steer" });
  const session = await drive<{ items: string[] }>(t, "drive=queue&kind=session");
  assert.deepEqual(session.items, ["Edit message", "Open in side chat", "Turn off queueing"]);
});

test("a finished answer's file chip holds still while a later turn works: never drawn again as plain code", { timeout: 120000 }, async (t) => {
  const seen = await drive<{ same: boolean; connected: boolean; events: string[]; color: string | null }>(t, "drive=filelink&kind=session");
  // Each event of the later turn moves the conversation's time, which reads the checkout again.
  assert.deepEqual(seen.events, []);
  assert.equal(seen.same, true);
  assert.equal(seen.connected, true);
  assert.ok(seen.color, "the chip is there, in link blue");
});

test("a finished answer names its worker's file once, inline: nothing under the answer, no Save by the link; it opens in the reader, with Save there, in a Chat and a session", { timeout: 180000 }, async (t) => {
  for (const kind of ["chat", "session"]) {
    const seen = await drive<{
      underAnswer: boolean; chips: (string | null)[]; linkLine: string | null; saveByLink: boolean;
      opened: { title: string | null; heading: string | null; items: (string | null)[]; save: string | null }; calls: string[];
    }>(t, `drive=answerfile&kind=${kind}`);
    assert.deepEqual([seen.underAnswer, seen.chips, seen.saveByLink], [false, ["notes.md"], false], kind);
    assert.equal(seen.linkLine, "Totals now round half-even, once, after tax. The cases I checked are in notes.md; the invoice test covers the two that failed.", kind);
    // Its Markdown as the answer shows it, and its Save.
    assert.deepEqual(seen.opened, {
      title: "notes.md",
      heading: "Rounding cases",
      items: ["2.345 rounds to 2.34 (half-even)", "2.355 rounds to 2.36", "Totals are rounded once, after tax."],
      save: "Save…",
    }, kind);
    assert.deepEqual(seen.calls.filter((call) => !call.startsWith("listFiles")), [
      "resolveFileLink outputs/task-1/notes.md", "readArtifact blob-notes", "save_artifact blob-notes",
    ], kind);
  }
});

test("an ending that opens with Update: is the answer, its own Details folded where it changed code, in a session and a Chat", { timeout: 180000 }, async (t) => {
  for (const kind of ["session", "chat"]) {
    const seen = await drive<{ answers: (string | null)[]; updates: number; quotes: number; details: { summary: string | null; open: boolean }[]; opening: boolean }>(
      t, `drive=update&kind=${kind}`);
    // One answer in view, down to its Details; no update row and no quote block under an opening line.
    const shown = "Update: the session's full checks still don't pass, so it isn't merged. The rounding fix itself is in.";
    assert.deepEqual(seen.answers, [kind === "session" ? `${shown} Details` : shown], kind);
    assert.deepEqual([seen.updates, seen.quotes, seen.opening], [0, 0, false], kind);
    // The model's own `### Details` folds like the app's where the turn changed code; a Chat's
    // turn changed none, so its answer has no Details (Q79).
    assert.deepEqual(seen.details, kind === "session" ? [{ summary: "Details", open: false }] : [], kind);
  }
});

test("a follow-up that joined a turn as it ended leaves the first answer in view, above its bubble, with its own actions", { timeout: 120000 }, async (t) => {
  const seen = await drive<{
    first: boolean; second: boolean; firstAboveBubble: boolean; header: string | null; firstActions: string[]; firstActionsOpacity: string | null;
  }>(t, "drive=joined&kind=session");
  assert.equal(seen.first, true, JSON.stringify(seen));
  assert.equal(seen.second, true, JSON.stringify(seen));
  assert.equal(seen.firstAboveBubble, true, JSON.stringify(seen));
  // Every reply has Copy and the others, not only the turn's last one (Q81).
  assert.ok(seen.firstActions.includes("Copy response"), JSON.stringify(seen.firstActions));
  assert.ok(seen.firstActions.includes("Feedback"), JSON.stringify(seen.firstActions));
  assert.equal(seen.firstActionsOpacity, "1");
});

test("a worker the thread starts is a team sentence among its work, folded with it once answered, never a card", { timeout: 120000 }, async (t) => {
  const seen = await drive<{
    workerLive: { shown: boolean; words: string | null; card: boolean };
    workerDone: { rows: (string | null)[]; shown: boolean; card: boolean };
    header: string | null;
  }>(t, "drive=joined&kind=session");
  assert.equal(seen.workerLive.shown, true, JSON.stringify(seen));
  assert.match(seen.workerLive.words ?? "", /Changelog check/);
  assert.equal(seen.workerLive.card, false, JSON.stringify(seen));
  // Answered, it is work under the closed "Worked for …" like the rest.
  assert.match(seen.header ?? "", /^Worked for/);
  assert.equal(seen.workerDone.shown, false, JSON.stringify(seen));
  assert.equal(seen.workerDone.card, false, JSON.stringify(seen));
});

