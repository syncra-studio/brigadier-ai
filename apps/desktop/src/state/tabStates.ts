import { create } from "zustand";

import { request } from "@/ipc/client";
import type { Conversation, EventEnvelope } from "@/ipc/generated";
import { type RowActivity, rowActivity, stoppedOf, useActivity, useRowActivity } from "@/state/activity";
import { useApp } from "@/state/store";

/** Git marks retained for the context card's merge/undo refresh; never used as row words. */
export type Settled = { state?: "ready" | "merged" | "reverted"; ahead?: number };
export const useSettledTabs = create<{ byTab: Readonly<Record<string, Settled>> }>(() => ({ byTab: {} }));

export async function loadSettledTabs(): Promise<void> {
  const { tabs } = await request({ method: "getSettledTabs" });
  useSettledTabs.setState({ byTab: Object.fromEntries(tabs.flatMap(({ conversationId, state, ahead }) =>
    state === "ready" || state === "merged" || state === "reverted" ? [[conversationId, { state, ...(ahead === undefined ? {} : { ahead }) }]] : [],
  )) });
}

let refreshing: ReturnType<typeof setTimeout> | null = null;
export function refreshSettledTabsSoon(): void {
  if (refreshing) clearTimeout(refreshing);
  refreshing = setTimeout(() => { refreshing = null; void loadSettledTabs().catch(() => {}); }, 3000);
}

export function applySettledTabEvents(batch: readonly EventEnvelope[]): void {
  let changed: Record<string, Settled> | null = null;
  let refresh = false;
  for (const { stream, event } of batch) {
    const id = event.type === "requestReady" ? event.conversationId : stream.startsWith("conversation:") ? stream.slice("conversation:".length) : null;
    const state = event.type === "requestReady" ? "ready" : event.type === "orchestratorStepped" && event.step.kind.type === "merged" ? "merged" : null;
    const all: Readonly<Record<string, Settled>> = changed ?? useSettledTabs.getState().byTab;
    if (id && state) {
      if (all[id]?.state !== state || (state === "merged" && all[id]?.ahead !== undefined)) {
        changed = { ...all, [id]: state === "merged" ? { state } : { ...all[id], state } };
      }
      refresh = true;
    } else if (id && event.type === "runStateChanged" && event.state === "idle" && ["merged", "reverted"].includes(all[id]?.state ?? "")) refresh = true;
  }
  if (changed) useSettledTabs.setState({ byTab: changed });
  if (refresh) refreshSettledTabsSoon();
}

/** The row and tab name activity only; git marks belong to the context card. */
export type SessionState = "needsYou" | "failed" | "working" | "stopped" | "done";

/** Strongest first. */
export const TAB_STATES: readonly SessionState[] = ["needsYou", "failed", "working", "stopped", "done"];

/** A state's words alone, and after a count of sessions. */
export const TAB_STATE_WORDS: Record<SessionState, { one: string; many: string }> = {
  needsYou: { one: "Needs you", many: "need you" },
  failed: { one: "Failed", many: "failed" },
  working: { one: "Working", many: "working" },
  stopped: { one: "Stopped", many: "stopped" },
  done: { one: "Done", many: "done" },
};

export const SESSION_STATUS_WORDS = Object.fromEntries(
  Object.entries(TAB_STATE_WORDS).map(([state, words]) => [state, words.one]),
) as Record<SessionState, string>;

export type SessionStatus = { state: SessionState; reason: string | null };

/** A waiting card wins over failures, live work, a stop and a recorded turn's end. */
export function tabState(activity: RowActivity, quotaWait: boolean, stopped = false, turnEnded = false): SessionState | null {
  if (activity.awaiting && activity.awaiting !== "failed") return "needsYou";
  if (activity.awaiting === "failed") return "failed";
  if (activity.running || quotaWait) return "working";
  if (stopped) return "stopped";
  return turnEnded ? "done" : null;
}

/** The displayed state with the waiting card or quota reset named on hover. */
export function sessionStatus(
  activity: RowActivity,
  quotaWait: { resetsAtMs: number | null } | null,
  stopped = false,
  turnEnded = false,
): SessionStatus | null {
  const state = tabState(activity, quotaWait !== null, stopped, turnEnded);
  if (!state) return null;
  let reason: string | null = null;
  if (state === "needsYou") {
    reason = activity.awaiting === "approval" ? "approval" : activity.awaiting === "input" ? "question" : "sign-in";
  } else if (state === "working" && quotaWait && !activity.running) {
    const at = quotaWait.resetsAtMs === null ? null : new Date(quotaWait.resetsAtMs);
    reason = at === null ? "waiting for a usage limit" : `waiting for reset ${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
  }
  return { state, reason };
}

/** Needs you keeps its dot while open; Done uses the unread dot, failures their own red mark. */
export function sidebarIndicators(activity: RowActivity, quotaWait: boolean, unread: boolean, stopped = false, turnEnded = false) {
  const state = tabState(activity, quotaWait, stopped, turnEnded);
  return { working: state === "working", error: state === "failed", unread: state === "needsYou" || (state === "done" && unread) };
}

export function useSidebarIndicators(id: string, unread = false) {
  const activity = useRowActivity(id);
  const quotaWait = useApp((s) => !!s.conversations[id]?.quotaWait);
  const stopped = useActivity((s) => stoppedOf(s.byConversation[id]));
  const turnEnded = useActivity((s) => s.byConversation[id]?.turnEndedAtMs !== undefined);
  return sidebarIndicators(activity, quotaWait, unread, stopped, turnEnded);
}

export function useSessionStatus(id: string): SessionStatus | null {
  const activity = useRowActivity(id);
  const resets = useApp((s) => {
    const wait = s.conversations[id]?.quotaWait;
    return wait ? (wait.resetsAtMs ?? NaN) : null;
  });
  const stopped = useActivity((s) => stoppedOf(s.byConversation[id]));
  const turnEnded = useActivity((s) => s.byConversation[id]?.turnEndedAtMs !== undefined);
  return sessionStatus(activity, resets === null ? null : { resetsAtMs: Number.isNaN(resets) ? null : resets }, stopped, turnEnded);
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
  const states = useActivity((s) => ids.map((id, index) => {
    const own = s.byConversation[id];
    return tabState(rowActivity(own), waiting[index] === "1", stoppedOf(own), own?.turnEndedAtMs !== undefined) ?? NONE;
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
