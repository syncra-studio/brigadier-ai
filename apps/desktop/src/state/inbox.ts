import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { forksUnderSources } from "@/app/conversation/forks";
import type { Conversation } from "@/ipc/generated";
import { useActivity } from "@/state/activity";
import { finishedAt } from "@/state/lastOpened";
import { useApp } from "@/state/store";
import { type SessionState, useTabStates } from "@/state/tabStates";

/** The inbox's five sections, top to bottom (D7). */
export const INBOX_SECTIONS = ["pinned", "active", "working", "snoozed", "done"] as const;
export type InboxSection = (typeof INBOX_SECTIONS)[number];
export type Inbox = Record<InboxSection, Conversation[]>;

/** How long a row that stops working stays on the Working shelf, so a blink doesn't move it. */
export const WORKING_GRACE_MS = 2000;

/**
 * Where a row goes (D7, D11): pinned first; its spinner puts it on the Working shelf (a new
 * message starts a turn, so this is also how one ends Done), and keeps it there for a moment after
 * unless it needs you or failed; Done and Snoozed hold until the thread moves on (a turn ends or
 * fails after it, or it needs you) and Snoozed until its time. Derived only: nothing is written on
 * wake.
 */
export function inboxSection(
  conversation: Conversation,
  state: SessionState | null,
  finishedAtMs: number | undefined,
  nowMs: number,
  leftWorkingAtMs?: number,
): InboxSection {
  if (conversation.pinnedAtMs !== null) return "pinned";
  if (state === "working") return "working";
  const blinked = leftWorkingAtMs !== undefined && nowMs - leftWorkingAtMs < WORKING_GRACE_MS;
  if (blinked && state !== "needsYou" && state !== "failed") return "working";
  const movedOnSince = (atMs: number) => state === "needsYou" || (finishedAtMs ?? 0) > atMs;
  if (conversation.doneAtMs !== undefined && !movedOnSince(conversation.doneAtMs)) return "done";
  const until = conversation.snoozedUntilMs;
  if (until !== undefined && until > nowMs && !movedOnSince(conversation.snoozedAtMs ?? 0)) return "snoozed";
  return "active";
}

/**
 * A row's place in Active and Working: created, finished, failed or left working (needs you, a new
 * message's turn). Not `updatedAtMs`: sleep, wake and renames move it, and must not move the row (D7).
 */
export function activeKey(conversation: Conversation, finishedAtMs: number | undefined, leftWorkingAtMs: number | undefined): number {
  return Math.max(conversation.createdAtMs, finishedAtMs ?? 0, leftWorkingAtMs ?? 0);
}

export type InboxRow = {
  conversation: Conversation;
  state: SessionState | null;
  finishedAtMs?: number | undefined;
  leftWorkingAtMs?: number | undefined;
};

/** Every row in its section and order; a fork stays under its source. */
export function inboxLists(rows: readonly InboxRow[], nowMs: number): Inbox {
  const inbox: Record<InboxSection, InboxRow[]> = { pinned: [], active: [], working: [], snoozed: [], done: [] };
  for (const row of rows) inbox[inboxSection(row.conversation, row.state, row.finishedAtMs, nowMs, row.leftWorkingAtMs)].push(row);
  const newest = (a: InboxRow, b: InboxRow) =>
    activeKey(b.conversation, b.finishedAtMs, b.leftWorkingAtMs) - activeKey(a.conversation, a.finishedAtMs, a.leftWorkingAtMs);
  const by = (list: InboxRow[], order: (a: InboxRow, b: InboxRow) => number) =>
    forksUnderSources(list.toSorted(order).map((row) => row.conversation));
  return {
    pinned: by(inbox.pinned, (a, b) => (b.conversation.pinnedAtMs ?? 0) - (a.conversation.pinnedAtMs ?? 0)),
    active: by(inbox.active, newest),
    working: by(inbox.working, newest),
    snoozed: by(inbox.snoozed, (a, b) => (a.conversation.snoozedUntilMs ?? 0) - (b.conversation.snoozedUntilMs ?? 0)),
    done: by(inbox.done, (a, b) => (b.conversation.doneAtMs ?? 0) - (a.conversation.doneAtMs ?? 0)),
  };
}

/** The inbox as the sidebar shows it, re-sorted every minute so due snoozes wake. */
export function useInbox(): Inbox {
  const conversations = useApp((s) => s.conversations);
  // Archived ones live in Settings → Archived; side and tab chats sit beside their session.
  const listed = useMemo(
    () => Object.values(conversations).filter((conversation) => conversation.lifecycle !== "archived" && !conversation.sideOf),
    [conversations],
  );
  const ids = useMemo(() => listed.map((conversation) => conversation.id), [listed]);
  const states = useTabStates(ids).join(" ");
  const ended = useActivity((s) => ids.map((id) => s.byConversation[id]?.turnEndedAtMs ?? 0).join(" "));
  const [nowMs, setNow] = useState(Date.now);
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(tick);
  }, []);

  // When each row last stopped working, in this window (memory only); set before paint so a row
  // that blinks out of Working never shows elsewhere.
  const [leftWorkingAt, setLeftWorkingAt] = useState<Readonly<Record<string, number>>>({});
  const before = useRef<Record<string, string>>({});
  useLayoutEffect(() => {
    const now = Date.now();
    const left: Record<string, number> = {};
    states.split(" ").forEach((state, index) => {
      const id = ids[index];
      if (id === undefined) return;
      if (before.current[id] === "working" && state !== "working") left[id] = now;
      before.current[id] = state;
    });
    if (Object.keys(left).length === 0) return;
    // oxlint-disable-next-line react/set-state-in-effect -- the leave time is the wall clock's, read once here, not derivable while rendering.
    setLeftWorkingAt((all) => ({ ...all, ...left }));
  }, [ids, states]);
  // The moment the last grace ends, look again.
  useEffect(() => {
    const ends = Math.max(0, ...Object.values(leftWorkingAt)) + WORKING_GRACE_MS - Date.now();
    if (ends <= 0) return;
    const timer = setTimeout(() => setNow(Date.now()), ends);
    return () => clearTimeout(timer);
  }, [leftWorkingAt]);

  return useMemo(() => {
    const shown = states.split(" ");
    const endedAt = ended.split(" ").map(Number);
    return inboxLists(
      listed.map((conversation, index) => ({
        conversation,
        state: shown[index] && shown[index] !== "-" ? (shown[index] as SessionState) : null,
        finishedAtMs: finishedAt(conversation, endedAt[index] || undefined),
        leftWorkingAtMs: leftWorkingAt[conversation.id],
      })),
      nowMs,
    );
  }, [listed, states, ended, leftWorkingAt, nowMs]);
}
