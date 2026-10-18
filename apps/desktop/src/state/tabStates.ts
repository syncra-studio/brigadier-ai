import { create } from "zustand";

import { request } from "@/ipc/client";
import type { Conversation, EventEnvelope, TabState } from "@/ipc/generated";
import { type RowActivity, rowActivity, useActivity } from "@/state/activity";
import { useApp } from "@/state/store";

/**
 * Where task tabs stand (WORKSPACES-PLAN.md "States"), as the strip and the sidebar's workspace
 * rows show it. Needs you and Working come from the activity the sidebar already folds; Ready and
 * Merged need git, so the daemon gives them at connect and its events keep them: a request that
 * settles ahead of base is ready, a merge of the tab's work is merged.
 */
export const useSettledTabs = create<{ byTab: Readonly<Record<string, "ready" | "merged">> }>(() => ({
  byTab: {},
}));

export async function loadSettledTabs(): Promise<void> {
  const { tabs } = await request({ method: "getSettledTabs" });
  useSettledTabs.setState({
    byTab: Object.fromEntries(
      tabs.flatMap(({ conversationId, state }) =>
        state === "ready" || state === "merged" ? [[conversationId, state] as const] : [],
      ),
    ),
  });
}

let refreshing: ReturnType<typeof setTimeout> | null = null;

/**
 * Reads the settled states again once a watched checkout has been quiet for a moment: a commit
 * or merge made by hand (Source, a terminal) has no event of its own.
 */
export function refreshSettledTabsSoon(): void {
  if (refreshing) clearTimeout(refreshing);
  refreshing = setTimeout(() => {
    refreshing = null;
    void loadSettledTabs().catch(() => {});
  }, 3000);
}

/** Folds a batch of events into the tabs' ready and merged marks. */
export function applySettledTabEvents(batch: readonly EventEnvelope[]): void {
  let changed: Record<string, "ready" | "merged"> | null = null;
  const mark = (id: string, state: "ready" | "merged") => {
    const current = changed ?? useSettledTabs.getState().byTab;
    if (current[id] === state) return;
    changed = { ...current, [id]: state };
  };
  for (const { stream, event } of batch) {
    if (event.type === "requestReady") mark(event.conversationId, "ready");
    else if (
      event.type === "orchestratorStepped" &&
      event.step.kind.type === "merged" &&
      stream.startsWith("conversation:")
    ) {
      mark(stream.slice("conversation:".length), "merged");
    }
  }
  if (changed) useSettledTabs.setState({ byTab: changed });
}

/** The first that holds wins: needs you, working, then the settled mark. */
export function tabState(
  activity: RowActivity,
  quotaWait: boolean,
  settled: "ready" | "merged" | undefined,
): TabState | null {
  if (quotaWait || activity.awaiting) return "needsYou";
  if (activity.running || activity.phase) return "working";
  return settled ?? null;
}

/** Strongest first. */
export const TAB_STATES: readonly TabState[] = ["needsYou", "working", "ready", "merged"];

/** A state's words alone ("Needs you"), and after a count of tabs ("2 need you"). */
export const TAB_STATE_WORDS: Record<TabState, { one: string; many: string }> = {
  needsYou: { one: "Needs you", many: "need you" },
  working: { one: "Working", many: "working" },
  ready: { one: "Ready", many: "ready" },
  merged: { one: "Merged", many: "merged" },
};

/** The strongest state among `states`, with how many tabs share it, or null when none has one. */
export function strongestState(
  states: readonly (TabState | null)[],
): { state: TabState; count: number } | null {
  for (const state of TAB_STATES) {
    const count = states.filter((each) => each === state).length;
    if (count > 0) return { state, count };
  }
  return null;
}

/** A row's words for a workspace state: "Working", or "2 working" when several tabs share it. */
export function stateWords({ state, count }: { state: TabState; count: number }): string {
  return count > 1 ? `${count} ${TAB_STATE_WORDS[state].many}` : TAB_STATE_WORDS[state].one;
}

/** One tab's state; re-renders only when it changes. */
export function useTabState(id: string): TabState | null {
  return useTabStates([id])[0] ?? null;
}

const NONE = "-";

/** The states of `ids`, in order; re-renders only when one changes. */
export function useTabStates(ids: readonly string[]): (TabState | null)[] {
  // Each store gives a string, so a change elsewhere renders nothing here.
  const live = useActivity((s) =>
    ids
      .map((id) => {
        const activity = rowActivity(s.byConversation[id]);
        return activity.awaiting ? "needsYou" : activity.running || activity.phase ? "working" : NONE;
      })
      .join(" "),
  );
  const waiting = useApp((s) => ids.map((id) => (s.conversations[id]?.quotaWait ? "1" : "0")).join(""));
  const settled = useSettledTabs((s) => ids.map((id) => s.byTab[id] ?? NONE).join(" "));
  const lives = live.split(" ");
  const settles = settled.split(" ");
  return ids.map((_, index) => {
    const mark = settles[index];
    return tabState(
      { running: lives[index] === "working", awaiting: lives[index] === "needsYou" ? "approval" : null, phase: null },
      waiting[index] === "1",
      mark === "ready" || mark === "merged" ? mark : undefined,
    );
  });
}

/** Whether `conversation` is a workspace's task tab the user can open. */
export function isTaskTab(conversation: Conversation): boolean {
  return (
    !!conversation.workspaceId &&
    conversation.kind === "session" &&
    !conversation.sideOf &&
    !conversation.tabChat &&
    conversation.lifecycle !== "archived"
  );
}

/** The task tabs of `workspaceId`, oldest first (the strip's order). */
export function workspaceTabs(
  conversations: Readonly<Record<string, Conversation>>,
  workspaceId: string,
): Conversation[] {
  return Object.values(conversations)
    .filter((conversation) => conversation.workspaceId === workspaceId && isTaskTab(conversation))
    .toSorted((a, b) => a.createdAtMs - b.createdAtMs);
}

/**
 * What picked sidebar rows stand for: a workspace's row is all its tabs the row groups (those
 * not pinned, which have rows of their own).
 */
export function rowTabs(conversations: Readonly<Record<string, Conversation>>, ids: readonly string[]): string[] {
  const tabs = new Set<string>();
  for (const id of ids) {
    tabs.add(id);
    const conversation = conversations[id];
    if (!conversation?.workspaceId || conversation.pinnedAtMs !== null || !isTaskTab(conversation)) continue;
    for (const tab of workspaceTabs(conversations, conversation.workspaceId)) {
      if (tab.pinnedAtMs === null) tabs.add(tab.id);
    }
  }
  return [...tabs];
}
