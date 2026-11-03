import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation } from "@/ipc/generated";
import { inboxLists, inboxSection } from "@/state/inbox";
import type { SessionState } from "@/state/tabStates";

const NOW = 1_000_000;
const row = (id: string, extra: Partial<Conversation> = {}): Conversation => ({
  id, kind: "session", projectId: "p", title: id, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, ...extra,
});

test("inboxSection: every row's section (D7, D11)", () => {
  const done = { doneAtMs: 500 };
  const snoozed = { snoozedUntilMs: NOW + 60_000, snoozedAtMs: 500 };
  const cases: [string, Partial<Conversation>, SessionState | null, number | undefined, string][] = [
    ["a quiet row", {}, null, undefined, "active"],
    ["a finished row, unread or not", {}, "done", 400, "active"],
    ["pinned wins over everything", { pinnedAtMs: 1, ...done }, "working", undefined, "pinned"],
    ["working: the spinner", {}, "working", undefined, "working"],
    ["needs you is not working: its dot wins (D11)", {}, "needsYou", undefined, "active"],
    ["failed", {}, "failed", 400, "active"],
    // Done.
    ["marked done", done, null, 400, "done"],
    ["done, its last turn failed before", done, "failed", 400, "done"],
    ["done, a new message started a turn", done, "working", 400, "working"],
    ["done, a turn ended after", done, "done", 600, "active"],
    ["done, a turn failed after", done, "failed", 600, "active"],
    ["done, it needs you", done, "needsYou", 400, "active"],
    ["auto-done (3 idle days or a merged PR) reads the same", { doneAtMs: NOW - 10 }, null, NOW - 3 * 86_400_000, "done"],
    // Snooze.
    ["snoozed", snoozed, null, 400, "snoozed"],
    ["snoozed, its time came", { ...snoozed, snoozedUntilMs: NOW }, null, 400, "active"],
    ["snoozed, failed before it", snoozed, "failed", 400, "snoozed"],
    ["early wake: needs you", snoozed, "needsYou", 400, "active"],
    ["early wake: a turn failed after", snoozed, "failed", 600, "active"],
    ["early wake: a message's turn ended after", snoozed, "done", 600, "active"],
    ["snoozed, a message's turn runs", snoozed, "working", 400, "working"],
    ["done beats snoozed", { ...done, ...snoozed }, null, 400, "done"],
  ];
  for (const [what, extra, state, finishedAtMs, section] of cases) {
    assert.equal(inboxSection(row("r", extra), state, finishedAtMs, NOW), section, what);
  }
});

test("inboxSection: a row that just stopped working stays on the shelf 2 s, unless it needs you or failed", () => {
  const justLeft = NOW - 500;
  assert.equal(inboxSection(row("r"), "done", NOW, NOW, justLeft), "working", "blinked out");
  assert.equal(inboxSection(row("r"), null, undefined, NOW, NOW - 2000), "active", "grace over");
  assert.equal(inboxSection(row("r"), "needsYou", undefined, NOW, justLeft), "active", "needs you goes at once");
  assert.equal(inboxSection(row("r"), "failed", NOW, NOW, justLeft), "active", "a failure goes at once");
});

test("inboxLists: newest first by its latest move; pins, snoozes and done by their own times; forks under sources", () => {
  const lists = inboxLists([
    { conversation: row("old"), state: null, finishedAtMs: 10 },
    { conversation: row("finished", { updatedAtMs: 5 }), state: "done", finishedAtMs: 300 },
    { conversation: row("needs-you", { updatedAtMs: 5 }), state: "needsYou", leftWorkingAtMs: 400 },
    { conversation: row("new", { createdAtMs: 200, updatedAtMs: 200 }), state: null },
    { conversation: row("fork", { createdAtMs: 350, updatedAtMs: 350, forkedFrom: { conversationId: "old", messageId: "m" } }), state: null },
    { conversation: row("pin-a", { pinnedAtMs: 1 }), state: "working" },
    { conversation: row("pin-b", { pinnedAtMs: 2 }), state: null },
    { conversation: row("run-a"), state: "working", finishedAtMs: 50 },
    { conversation: row("run-b"), state: "working", finishedAtMs: 60 },
    // Sleep and wake only move updatedAtMs: the row stays where it was (D7).
    { conversation: row("slept", { updatedAtMs: 999 }), state: null, finishedAtMs: 5 },
    { conversation: row("wake-late", { snoozedUntilMs: NOW + 9, snoozedAtMs: 1 }), state: null },
    { conversation: row("wake-soon", { snoozedUntilMs: NOW + 1, snoozedAtMs: 1 }), state: null },
    { conversation: row("done-early", { doneAtMs: 100 }), state: null },
    { conversation: row("done-late", { doneAtMs: 200 }), state: null },
  ], NOW);
  const ids = Object.fromEntries(Object.entries(lists).map(([section, list]) => [section, list.map((c) => c.id)]));
  assert.deepEqual(ids, {
    pinned: ["pin-b", "pin-a"],
    active: ["needs-you", "finished", "new", "old", "fork", "slept"],
    working: ["run-b", "run-a"],
    snoozed: ["wake-soon", "wake-late"],
    done: ["done-late", "done-early"],
  });
});
