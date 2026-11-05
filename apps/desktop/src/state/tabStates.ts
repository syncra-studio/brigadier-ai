import { create } from "zustand";

import { request } from "@/ipc/client";
import type { Conversation, EventEnvelope, SettledTab } from "@/ipc/generated";
import { type RowActivity, rowActivity, stoppedOf, useActivity, useRowActivity } from "@/state/activity";
import { unread as isUnread, useFinishedAt } from "@/state/lastOpened";
import { useApp } from "@/state/store";

/**
 * Git marks retained for the context card's merge/undo refresh; never used as row words.
 * `landed`: nothing left to merge, committed or not (D61); absent until git was asked.
 */
export type Settled = { state?: "ready" | "merged" | "reverted"; ahead?: number; landed?: boolean; prMerged?: boolean };
export const useSettledTabs = create<{ byTab: Readonly<Record<string, Settled>> }>(() => ({ byTab: {} }));

/** The marks by tab from the daemon's settled tabs, the sessions with nothing left to merge and those whose pull request merged. */
export function settledByTab(tabs: readonly SettledTab[], landed: readonly string[], merged: readonly string[] = []): Record<string, Settled> {
  const byTab: Record<string, Settled> = Object.fromEntries(tabs.flatMap(({ conversationId, state, ahead }) =>
    state === "ready" || state === "merged" || state === "reverted" ? [[conversationId, { state, ...(ahead === undefined ? {} : { ahead }), landed: landed.includes(conversationId) }]] : [],
  ));
  for (const id of landed) byTab[id] ??= { landed: true };
  for (const id of merged) byTab[id] = { ...byTab[id], prMerged: true };
  return byTab;
}

export async function loadSettledTabs(): Promise<void> {
  const { tabs, landed, merged } = await request({ method: "getSettledTabs" });
  useSettledTabs.setState({ byTab: settledByTab(tabs, landed, merged) });
}

let refreshing: ReturnType<typeof setTimeout> | null = null;
export function refreshSettledTabsSoon(): void {
  if (refreshing) clearTimeout(refreshing);
  refreshing = setTimeout(() => { refreshing = null; void loadSettledTabs().catch(() => {}); }, 3000);
}

/** Whether `conversation` is a session on a branch of its own, which its thread may merge or push. */
function onOwnBranch(conversation: Conversation | undefined): boolean {
  return conversation?.setup?.type === "session" && conversation.setup.environment.type === "newWorktree";
}

/**
 * Keeps the git marks current with the events: a ready request or a merge marks its tab, and the
 * turn end of a marked tab or of a session on a branch of its own looks at git again (its thread
 * may have merged or pushed its work itself, D61). Whether it will look again.
 */
export function applySettledTabEvents(batch: readonly EventEnvelope[]): boolean {
  let changed: Record<string, Settled> | null = null;
  let refresh = false;
  for (const { stream, event } of batch) {
    if (event.type === "pullRequestSeen") {
      // The Merged chip, from what the board just saw: no look at git needed.
      const all: Readonly<Record<string, Settled>> = changed ?? useSettledTabs.getState().byTab;
      const prMerged = event.pullRequest?.state === "merged";
      if ((all[event.conversationId]?.prMerged ?? false) !== prMerged) {
        changed = { ...all, [event.conversationId]: { ...all[event.conversationId], prMerged } };
      }
      continue;
    }
    const id = event.type === "requestReady" ? event.conversationId : stream.startsWith("conversation:") ? stream.slice("conversation:".length) : null;
    const state = event.type === "requestReady" ? "ready" : event.type === "orchestratorStepped" && event.step.kind.type === "merged" ? "merged" : null;
    const all: Readonly<Record<string, Settled>> = changed ?? useSettledTabs.getState().byTab;
    if (id && state) {
      if (all[id]?.state !== state || (state === "merged" && all[id]?.ahead !== undefined)) {
        changed = { ...all, [id]: state === "merged" ? { state } : { ...all[id], state } };
      }
      refresh = true;
    } else if (id && event.type === "runStateChanged" && event.state === "idle" && (all[id] !== undefined || onOwnBranch(useApp.getState().conversations[id]))) refresh = true;
  }
  if (changed) useSettledTabs.setState({ byTab: changed });
  if (refresh) refreshSettledTabsSoon();
  return refresh;
}

/** The row and tab name activity only; git marks belong to the context card. */
export type SessionState = "needsYou" | "failed" | "working" | "stopped" | "done";

/** Strongest first. */
export const TAB_STATES: readonly SessionState[] = ["needsYou", "failed", "working", "stopped", "done"];

/** A state's words alone, and after a count of sessions. */
export const TAB_STATE_WORDS: Record<Exclude<SessionState, "done">, { one: string; many: string }> = {
  needsYou: { one: "Needs you", many: "need you" },
  failed: { one: "Failed", many: "failed" },
  working: { one: "Working", many: "working" },
  stopped: { one: "Stopped", many: "stopped" },
};

/**
 * A waiting card wins over failures, live work, a stop and a recorded turn's end; a sign-in only
 * while nothing works, as the session goes on without it. Not for the session the user is looking
 * at (`viewing`): its card is in front of them.
 */
export function tabState(activity: RowActivity, quotaWait: boolean, stopped = false, turnEnded = false, viewing = false): SessionState | null {
  if (needsYou(activity, viewing)) return "needsYou";
  if (activity.awaiting === "failed") return "failed";
  if (activity.running || quotaWait) return "working";
  if (stopped) return "stopped";
  return turnEnded ? "done" : null;
}

function needsYou({ awaiting, running }: RowActivity, viewing: boolean): boolean {
  if (viewing || !awaiting || awaiting === "failed") return false;
  return awaiting !== "signIn" || !running;
}


/** A row's mark: a spinner, the red error mark, the blue dot, or nothing; never a word (D3). */
export type RowLook = "working" | "error" | "dot" | null;
/**
 * On a sidebar row, `waiting` (a question, approval or sign-in) draws the blue ring and `amber` (a
 * failed run) the amber one, whatever `look` says; a chat tab goes by `look` alone.
 */
export type RowMark = { look: RowLook; hover: string | null; waiting?: boolean; amber?: boolean };

const NO_MARK: RowMark = { look: null, hover: null };

const hhmm = (atMs: number) => {
  const at = new Date(atMs);
  return `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
};

/**
 * The one mark of a row, tab or flyout (D4, D9, D11): needs you and an unread reply share the dot,
 * the hover says which. Stopped, asleep with nothing unread, or read: nothing (D10, D12).
 */
export function rowMark(
  activity: RowActivity,
  { quotaWait = null, stopped = false, unread = false, viewing = false }: {
    quotaWait?: { resetsAtMs: number | null } | null;
    stopped?: boolean;
    unread?: boolean;
    viewing?: boolean;
  } = {},
): RowMark {
  switch (tabState(activity, quotaWait !== null, stopped, false, viewing)) {
    case "needsYou":
      return { look: "dot", waiting: true, hover: `Needs you · ${activity.awaiting === "approval" ? "approval" : activity.awaiting === "input" ? "question" : "sign-in"}` };
    case "failed":
      return { look: "error", amber: true, hover: "Failed" };
    case "working":
      if (quotaWait && !activity.running) {
        return { look: "working", hover: quotaWait.resetsAtMs === null ? "Waiting for usage limit" : `Waiting for usage limit · resets ${hhmm(quotaWait.resetsAtMs)}` };
      }
      return { look: "working", hover: "Working" };
    case "stopped":
      return NO_MARK;
    default:
      return unread ? { look: "dot", hover: "New reply" } : NO_MARK;
  }
}

/** `id`'s mark, from the activity store, its usage wait and the unread clock. */
export function useRowMark(id: string): RowMark {
  const activity = useRowActivity(id);
  const resets = useApp((s) => {
    const wait = s.conversations[id]?.quotaWait;
    return wait ? (wait.resetsAtMs ?? NaN) : null;
  });
  const stopped = useActivity((s) => stoppedOf(s.byConversation[id]));
  const viewing = useViewing(id);
  const finished = useFinishedAt(id);
  const seen = useApp((s) => s.conversations[id]?.seenAtMs);
  const quotaWait = resets === null ? null : { resetsAtMs: Number.isNaN(resets) ? null : resets };
  return rowMark(activity, { quotaWait, stopped, unread: isUnread(finished, seen, viewing), viewing });
}

/** Whether `id` is the conversation in front of the user. */
function useViewing(id: string): boolean {
  return useApp((s) => s.selection.type === "conversation" && s.selection.id === id);
}

export function stillGoing(id: string): boolean {
  const state = tabState(rowActivity(useActivity.getState().byConversation[id]), !!useApp.getState().conversations[id]?.quotaWait);
  return state === "working" || state === "needsYou";
}

export function useTabState(id: string): SessionState | null {
  return useTabStates([id])[0] ?? null;
}

const NONE = "-";

export function useTabStates(ids: readonly string[]): (SessionState | null)[] {
  const waiting = useApp((s) => ids.map((id) => (s.conversations[id]?.quotaWait ? "1" : "0")).join(""));
  const viewing = useApp((s) => (s.selection.type === "conversation" ? s.selection.id : null));
  const states = useActivity((s) => ids.map((id, index) => {
    const own = s.byConversation[id];
    return tabState(rowActivity(own), waiting[index] === "1", stoppedOf(own), own?.turnEndedAtMs !== undefined, id === viewing) ?? NONE;
  }).join(" "));
  return states === "" ? [] : states.split(" ").map((state) => state === NONE ? null : state as SessionState);
}

/** Whether `conversation` is a session of a project the user can open: a sidebar row of its own. */
export function isTaskTab(conversation: Conversation): boolean {
  return (
    !!conversation.projectId &&
    conversation.kind === "session" &&
    !conversation.sideOf &&
    !conversation.tabChat &&
    conversation.lifecycle !== "archived"
  );
}
