import type { Selection } from "@/state/store";
import type { TerminalTabState } from "@/state/sessionTabs";
import type { WorkspaceTarget } from "@/state/workspaceCache";

/**
 * Tabs before a conversation exists: they live under the id the right sidebar already gives a
 * draft (`draft:<projectId>`), or `draft:none` for a new chat, and move to the conversation when
 * it is made (`carryDraftTabs`). Everything a tab reads goes through its owner and checkout here.
 */
export const CHAT_DRAFT_TABS = "draft:none";

/** Whose tabs a selection shows: its conversation's or its draft's. */
export function tabsIdOf(selection: Selection): string | null {
  if (selection.type === "conversation") return selection.id;
  if (selection.type !== "draft") return null;
  return selection.kind === "session" ? `draft:${selection.projectId}` : CHAT_DRAFT_TABS;
}

export function isDraftTabs(owner: string): boolean {
  return owner.startsWith("draft:");
}

/** The project of a draft's tabs; none for a new chat's, or for a conversation's. */
export function draftProject(owner: string): string | null {
  return isDraftTabs(owner) && owner !== CHAT_DRAFT_TABS ? owner.slice("draft:".length) : null;
}

/** What a tab of `owner` reads: the checkout it names, else its owner's. */
export function tabTarget(owner: string, checkout?: string): WorkspaceTarget {
  if (!isDraftTabs(owner)) return { type: "conversation", conversationId: owner };
  const projectId = draftProject(owner) ?? "";
  return checkout ? { type: "worktree", projectId, path: checkout } : { type: "project", projectId };
}

/** Where a checkout's rows open their tabs: whose they are, and the worktree they name if any. */
export function tabsOfTarget(target: WorkspaceTarget): { owner: string; checkout: string | undefined } {
  switch (target.type) {
    case "conversation": return { owner: target.conversationId, checkout: undefined };
    case "project": return { owner: `draft:${target.projectId}`, checkout: undefined };
    case "worktree": return { owner: `draft:${target.projectId}`, checkout: target.path };
  }
}

/**
 * Who the daemon keeps a terminal tab's shell under. Before a conversation exists a shell is
 * Home's, in the project's folder; a tab carried into a session keeps naming it, so the same shell
 * goes on running where it started.
 */
export function shellOwner(owner: string, tab: TerminalTabState): { conversationId: string } | { projectId: string } | Record<string, never> {
  if (tab.homeShell) return tab.homeShell.projectId ? { projectId: tab.homeShell.projectId } : {};
  if (!isDraftTabs(owner)) return { conversationId: owner };
  const projectId = draftProject(owner);
  return projectId ? { projectId } : {};
}
