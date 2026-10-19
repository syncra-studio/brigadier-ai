import { create } from "zustand";

import { request } from "@/ipc/client";
import type { Conversation, EventEnvelope, PullRequestState, TabState } from "@/ipc/generated";
import { type RowActivity, rowActivity, useActivity, useRowActivity } from "@/state/activity";
import { useApp } from "@/state/store";

/** What a session's row knows beyond its activity: its settled state, and why it reads so. */
export type Settled = {
  state?: "ready" | "merged" | "reverted";
  /** Its branch's pull request as last seen on GitHub. */
  pullRequest?: PullRequestState;
  /** The thread attached proof since its last merge. */
  proof?: boolean;
};

/**
 * Where sessions stand (WORKSPACES-PLAN.md "States"), as their chat tab and sidebar rows show
 * it. Needs you and Working come from the activity the sidebar already folds; Ready, Merged and
 * Reverted need git, so the daemon gives them at connect and its events keep them: a request
 * that settles ahead of base is ready, a merge of the session's work is merged. Its pull
 * request and proof come the same way.
 */
export const useSettledTabs = create<{ byTab: Readonly<Record<string, Settled>> }>(() => ({
  byTab: {},
}));

export async function loadSettledTabs(): Promise<void> {
  const { tabs } = await request({ method: "getSettledTabs" });
  useSettledTabs.setState({
    byTab: Object.fromEntries(
      tabs.flatMap(({ conversationId, state, pullRequest, proof }) =>
        state === "ready" || state === "merged" || state === "reverted"
          ? [[conversationId, { state, ...(pullRequest ? { pullRequest } : {}), ...(proof ? { proof } : {}) }] as const]
          : [],
      ),
    ),
  });
}

let refreshing: ReturnType<typeof setTimeout> | null = null;

/**
 * Reads the settled states again once a watched checkout has been quiet for a moment: a commit
 * or merge made by hand (Source, a terminal) has no event of its own, nor does a revert.
 */
export function refreshSettledTabsSoon(): void {
  if (refreshing) clearTimeout(refreshing);
  refreshing = setTimeout(() => {
    refreshing = null;
    void loadSettledTabs().catch(() => {});
  }, 3000);
}

/** Folds a batch of events into the sessions' settled marks and their reasons. */
export function applySettledTabEvents(batch: readonly EventEnvelope[]): void {
  let changed: Record<string, Settled> | null = null;
  let refresh = false;
  const update = (id: string, next: (current: Settled) => Settled) => {
    const all = changed ?? useSettledTabs.getState().byTab;
    const current = all[id] ?? {};
    const value = next(current);
    if (value.state === current.state && value.pullRequest === current.pullRequest && value.proof === current.proof) return;
    changed = { ...all, [id]: value };
  };
  for (const { stream, event } of batch) {
    const id = stream.startsWith("conversation:") ? stream.slice("conversation:".length) : null;
    if (event.type === "requestReady") update(event.conversationId, (current) => ({ ...current, state: "ready" }));
    else if (event.type === "pullRequestSeen") {
      const state = event.pullRequest?.state;
      update(event.conversationId, ({ pullRequest: _old, ...current }) => (state ? { ...current, pullRequest: state } : current));
    } else if (event.type === "orchestratorStepped" && id) {
      if (event.step.kind.type === "merged") update(id, (current) => ({ ...current, state: "merged", proof: false }));
      else if (event.step.kind.type === "proof") update(id, (current) => ({ ...current, proof: true }));
    } else if (event.type === "runStateChanged" && event.state === "idle") {
      // A turn may have reverted its merge on the base: git tells.
      const state = (changed ?? useSettledTabs.getState().byTab)[event.conversationId]?.state;
      if (state === "merged" || state === "reverted") refresh = true;
    }
  }
  if (changed) useSettledTabs.setState({ byTab: changed });
  if (refresh) refreshSettledTabsSoon();
}

/**
 * The first that holds wins, as the daemon's `Board::tab_state` decides it: needs you (a card,
 * a question, or its last turn failed, unless it waits for a limit), merging, working (a turn,
 * a worker, a phase, or its messages wait for a limit to reset), then the settled mark.
 */
export function tabState(activity: RowActivity, quotaWait: boolean, settled: Settled["state"]): TabState | null {
  if (activity.awaiting === "approval" || activity.awaiting === "input") return "needsYou";
  if (activity.awaiting === "failed" && !quotaWait) return "needsYou";
  if (activity.phase === "merging") return "merging";
  if (activity.running || activity.phase || quotaWait) return "working";
  return settled ?? null;
}

/** Strongest first. */
export const TAB_STATES: readonly TabState[] = ["needsYou", "merging", "working", "ready", "merged", "reverted"];

/** A state's words alone ("Needs you"), and after a count of sessions ("2 need you"). */
export const TAB_STATE_WORDS: Record<TabState, { one: string; many: string }> = {
  needsYou: { one: "Needs you", many: "need you" },
  working: { one: "Working", many: "working" },
  ready: { one: "Ready", many: "ready" },
  merging: { one: "Merging", many: "merging" },
  merged: { one: "Merged", many: "merged" },
  reverted: { one: "Reverted", many: "reverted" },
};

/** Display words preserve the engine's state ordering while naming failures and open PRs. */
export const SESSION_STATUS_WORDS = {
  ...Object.fromEntries(Object.entries(TAB_STATE_WORDS).map(([state, words]) => [state, words.one])) as Record<TabState, string>,
  prOpen: "PR open",
  failed: "Failed",
  stopped: "Stopped",
};

/** A session's displayed state and, when there is one, why. */
export type SessionStatus = { state: keyof typeof SESSION_STATUS_WORDS; reason: string | null };

const twoDigits = (value: number) => String(value).padStart(2, "0");

/** A time of day as HH:MM. */
function clock(ms: number): string {
  const at = new Date(ms);
  return `${twoDigits(at.getHours())}:${twoDigits(at.getMinutes())}`;
}

/**
 * Its state with the reason its row's tip names: Working · watching CI / · waiting for reset
 * HH:MM, Needs you · approval / · question, Failed, Stopped, PR open · proof attached. How many
 * workers it runs is never a reason: workers show in the session itself, not its row or tab.
 * `quotaWait`: its messages wait for a limit, which resets at the time given (null: unknown).
 */
export function sessionStatus(
  activity: RowActivity,
  quotaWait: { resetsAtMs: number | null } | null,
  settled: Settled | undefined,
  stopped = false,
): SessionStatus | null {
  let state: SessionStatus["state"] | null = tabState(activity, quotaWait !== null, settled?.state);
  if (state === "needsYou" && activity.awaiting === "failed") state = "failed";
  if (state !== "needsYou" && state !== "working" && state !== "merging" && state !== "failed") {
    if (stopped) state = "stopped";
    else if (state !== "merged" && state !== "reverted" && (settled?.pullRequest === "open" || settled?.pullRequest === "draft")) state = "prOpen";
  }
  if (!state) return null;
  let reason: string | null = null;
  if (state === "needsYou") {
    reason = activity.awaiting === "approval" ? "approval" : activity.awaiting === "input" ? "question" : "failed";
  } else if (state === "working") {
    if (activity.phase === "watchingCi") reason = "watching CI";
    else if (activity.phase === "reviewing") reason = "reviewing";
    else if (activity.phase === "checking") reason = "checking";
    else if (quotaWait && !activity.running) {
      reason = quotaWait.resetsAtMs === null ? "waiting for a usage limit" : `waiting for reset ${clock(quotaWait.resetsAtMs)}`;
    }
  } else if ((state === "ready" || state === "prOpen") && settled?.proof) {
    reason = "proof attached";
  }
  return { state, reason };
}

/** One session's state and reason; re-renders only when they change. */
export function useSessionStatus(id: string): SessionStatus | null {
  const activity = useRowActivity(id);
  const resets = useApp((s) => {
    const wait = s.conversations[id]?.quotaWait;
    return wait ? (wait.resetsAtMs ?? NaN) : null;
  });
  const settled = useSettledTabs((s) => s.byTab[id]);
  const stopped = useActivity((s) => s.byConversation[id]?.stopped ?? false);
  const status = sessionStatus(
    activity,
    resets === null ? null : { resetsAtMs: Number.isNaN(resets) ? null : resets },
    settled,
    stopped,
  );
  return status;
}

/** Whether the session `id` still has something going: it works, merges or needs the user. */
export function stillGoing(id: string): boolean {
  const activity = rowActivity(useActivity.getState().byConversation[id]);
  const state = tabState(activity, !!useApp.getState().conversations[id]?.quotaWait, undefined);
  return state === "working" || state === "merging" || state === "needsYou";
}

/** One session's state; re-renders only when it changes. */
export function useTabState(id: string): TabState | null {
  return useTabStates([id])[0] ?? null;
}

const NONE = "-";

/** The states of `ids`, in order; re-renders only when one changes. */
export function useTabStates(ids: readonly string[]): (TabState | null)[] {
  // Each store gives a string, so a change elsewhere renders nothing here.
  const waiting = useApp((s) => ids.map((id) => (s.conversations[id]?.quotaWait ? "1" : "0")).join(""));
  const live = useActivity((s) =>
    ids.map((id, index) => tabState(rowActivity(s.byConversation[id]), waiting[index] === "1", undefined) ?? NONE).join(" "),
  );
  const settled = useSettledTabs((s) => ids.map((id) => s.byTab[id]?.state ?? NONE).join(" "));
  const lives = live.split(" ");
  const settles = settled.split(" ");
  return ids.map((_, index) => {
    const own = lives[index];
    if (own && own !== NONE) return own as TabState;
    const mark = settles[index];
    return mark === "ready" || mark === "merged" || mark === "reverted" ? mark : null;
  });
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
