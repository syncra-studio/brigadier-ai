import assert from "node:assert/strict";
import { test } from "node:test";

import { commandsWaitWords, headerLabel } from "@/app/conversation/requestTime";
import { turnCommands } from "@/app/conversation/turnCommands";
import type { BackgroundTask, FileEntry, ThreadView } from "@/ipc/generated";

const task = (id: string, itemId: string | null, startedAtMs: number, more: Partial<BackgroundTask> = {}): BackgroundTask => ({
  id, itemId, kind: "shell", label: id, status: "running", startedAtMs, endedAtMs: null, outputFile: null, summary: null, lastLine: null, ...more,
});
/** Claude's Bash call: a `command` item, as its thread holds it (D38). */
const bash = (itemId: string, command: string, atMs: number): FileEntry => ({
  event: { type: "command", itemId, command, cwd: null, status: "completed", exitCode: null, output: null, durationMs: null },
  atMs,
});
const codex = (itemId: string, command: string, atMs: number): FileEntry => ({
  event: { type: "command", itemId, command, cwd: null, status: "completed", exitCode: null, output: null, durationMs: null },
  atMs,
});

const turns = [{ startedAtMs: 1000 }, { startedAtMs: 5000 }];

test("a background command holds the block whose turn started it, by its call, not by when it was seen (D21)", () => {
  const view: Pick<ThreadView, "background" | "entries" | "jobs"> = {
    entries: [bash("toolu_watch", "gh run watch 12 --exit-status", 1200), codex("item_ci", "gh pr checks 7 --watch", 1300)],
    // Codex lists its terminal only when it polls: seen after the next turn began.
    background: [task("bwatch", "toolu_watch", 1250), task("proc-9", "item_ci", 5200, { kind: "terminal", label: "gh pr checks 7 --watch" })],
    jobs: { holding: ["bwatch", "proc-9"] },
  };
  assert.equal(turnCommands(view, turns, [1000]), 2, "both started in the first turn");
  assert.equal(turnCommands(view, turns, [5000]), 0, "the later turn started neither");
  // A task with no call in the thread falls back to when it started.
  assert.equal(turnCommands({ ...view, entries: [] }, turns, [5000]), 1);
  // Only what the engine says holds: a wait, a preview call, a subagent never do.
  assert.equal(turnCommands({ ...view, jobs: { holding: ["bwatch"] } }, turns, [1000]), 1);
  assert.equal(turnCommands({ ...view, jobs: {} }, turns, [1000]), 0);
  assert.equal(turnCommands(view, turns, []), 0, "a live turn isn't held");
});

test("a block that starts with its message, before its CLI's turn began, still holds the command its turn started", () => {
  const view: Pick<ThreadView, "background" | "entries" | "jobs"> = {
    entries: [bash("toolu_sleep", "sleep 25 && echo done", 1200)],
    background: [task("bsleep", "toolu_sleep", 1250)],
    jobs: { holding: ["bsleep"] },
  };
  // Sent at 900, its turn began at 1000: the block's first span starts at the message.
  assert.equal(turnCommands(view, turns, [900]), 1);
  assert.equal(turnCommands(view, turns, [4900]), 0, "the next block's turn isn't this one's");
});

test("a command the next turn started isn't held by the block before it while that turn isn't listed yet", () => {
  // The newest listed turn began at 5000 and ended at 6000; the next one's call already came at 9500.
  const view: Pick<ThreadView, "background" | "entries" | "jobs"> = {
    entries: [bash("toolu_next", "sleep 5 && echo done", 9500)],
    background: [task("bnext", "toolu_next", 9550)],
    jobs: { holding: ["bnext"] },
  };
  assert.equal(turnCommands(view, turns, [5000], 6000), 0);
  assert.equal(turnCommands(view, turns, [5000], 9600), 1, "one its own turn started still holds it");
});

test("a command that ended holds the newest block that started it until the turn its end starts begins (D21)", () => {
  const ended = task("bwatch", "toolu_watch", 5250, { status: "completed", endedAtMs: 9000 });
  const waiting = task("bwait", "toolu_wait", 5260, { status: "completed", endedAtMs: 9000 });
  const entries = [bash("toolu_watch", "gh run watch 12", 5200), bash("toolu_wait", "brigadier start fix && brigadier wait", 5210)];
  assert.equal(turnCommands({ entries, background: [ended], jobs: { waking: true } }, turns, [5000]), 1);
  assert.equal(turnCommands({ entries, background: [ended], jobs: { waking: true } }, turns, [1000]), 0, "an older block isn't opened again");
  assert.equal(turnCommands({ entries, background: [ended], jobs: {} }, turns, [5000]), 0, "its turn began, or the grace passed");
  // A wait's end starts a turn as any command's does: it holds until that turn begins (D153).
  assert.equal(turnCommands({ entries, background: [waiting], jobs: { waking: true } }, turns, [5000]), 1);
});

test("a command that ended inside the block's last turn holds it while the turn its end starts is owed (D153, seen live on Codex)", () => {
  // Codex: "pause it" turn 5000-6000; the background `brigadier wait` ended at 5800 inside it, and
  // Codex began a turn for that end 0.4 s after this one ended: the block never reads over between.
  const wait = task("proc-3", "item_wait", 1300, { kind: "terminal", status: "completed", endedAtMs: 5800 });
  const entries = [codex("item_wait", "brigadier wait", 1200)];
  const view = (waking: boolean): Pick<ThreadView, "background" | "entries" | "jobs"> => ({ entries, background: [wait], jobs: { waking } });
  // The block's turns: 1000 (it started the wait) and 5000, its last, which ended at 6000.
  assert.equal(turnCommands(view(true), [{ startedAtMs: 1000 }, { startedAtMs: 5000 }], [1000, 5000], 6000), 1);
  assert.equal(turnCommands(view(false), [{ startedAtMs: 1000 }, { startedAtMs: 5000 }], [1000, 5000], 6000), 0, "the turn began, or the settle passed");
  // One that ended before the block's last turn began had its turn already.
  assert.equal(turnCommands(view(true), [{ startedAtMs: 1000 }, { startedAtMs: 7000 }], [1000, 7000], 8000), 0);
});

test("a held turn's live line says it waits for its commands; its header keeps only the time", () => {
  assert.equal(commandsWaitWords(1), "Waiting for a command");
  assert.equal(commandsWaitWords(2), "Waiting for 2 commands");
  assert.equal(headerLabel("working", 240_000), "Working for 4m 0s");
});

test("a block waiting on its question card reads Waiting for the wait so far; over, Worked for its work · waited for its waits (D155, D161)", async () => {
  const { cardWait } = await import("./requestTime");
  const asked = { createdAtMs: 10_000, answer: null, answeredAtMs: null };
  const waiting = cardWait([asked], 25_000);
  assert.deepEqual(waiting, { total: 15_000, open: 15_000 });
  assert.equal(headerLabel("waiting", 40_000, false, false, waiting), "Waiting for 15s");
  // Answered at 30 s, the block went on and ended at 70 s on the clock.
  const answered = cardWait([{ ...asked, answer: "yes" as never, answeredAtMs: 30_000 }], 99_000);
  assert.deepEqual(answered, { total: 20_000, open: 0 });
  assert.equal(headerLabel("working", 50_000, false, false, answered), "Working for 50s");
  assert.equal(headerLabel("done", 70_000, false, false, answered), "Worked for 50s · waited for 20s");
  // No card: as before.
  assert.equal(headerLabel("done", 70_000, false, false, cardWait([], 99_000)), "Worked for 1m 10s");
});

test("a held turn's one wait line names the user first: a question's answer or an approval, then workers, then commands", async () => {
  const { liveHead } = await import("./liveStatus");
  const answer = { text: "Waiting for your answer", tone: "needsYou" as const, waitsOn: "answer" as const };
  // The question card's own row says it waits ("Asking questions · waiting"): no second line (D161).
  assert.equal(liveHead(answer, 0, 1), null);
  assert.equal(liveHead(answer, 2, 1), null);
  const approval = { text: "Awaiting approval", tone: "needsYou" as const, waitsOn: "approval" as const };
  assert.equal(liveHead(approval, 2, 1)?.text, "Awaiting approval");
  assert.equal(liveHead({ text: "Thinking", tone: "busy" }, 0, 1)?.text, "Waiting for a command");
  assert.equal(liveHead({ text: "Thinking", tone: "busy" }, 1, 1)?.text, "Waiting for a worker");
  assert.equal(liveHead(null, 0, 0), null);
  // Only a subagent holds it: it waits on it, never "Thinking" (D104).
  assert.equal(liveHead({ text: "Thinking", tone: "busy" }, 0, 0, 1)?.text, "Waiting for a subagent");
});
