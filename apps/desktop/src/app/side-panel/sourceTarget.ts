import { create } from "zustand";

import { request } from "@/ipc/client";
import type { CommitOutcome, SourceChange, SourceState } from "@/ipc/generated";
import { openWorkspacePane } from "@/state/workspacePane";
import type { WorkspaceTarget } from "@/state/workspaceCache";

/** Whose changes the panel shows: a session's checkout, or the project folder of one not started. */
export type SourceTarget =
  | { type: "conversation"; conversationId: string }
  | { type: "project"; projectId: string; readOnly: boolean };

/** The panel's changes, sent for its target. */
export function sourceCalls(target: SourceTarget) {
  return target.type === "conversation"
    ? sessionCalls(target.conversationId)
    : projectCalls(target.projectId);
}

function sessionCalls(conversationId: string) {
  return {
    change: (change: SourceChange): Promise<{ state: SourceState }> =>
      change.type === "stage"
        ? request({ method: "stageFiles", conversationId, paths: change.paths })
        : change.type === "unstage"
          ? request({ method: "unstageFiles", conversationId, paths: change.paths })
          : request({ method: "discardFiles", conversationId, scope: change.scope, paths: change.paths }),
    commit: (message: string | null, push: boolean, amend: boolean): Promise<{ outcome: CommitOutcome }> =>
      request({ method: "commitChanges", conversationId, message, includeUnstaged: false, push, amend }),
  };
}

function projectCalls(projectId: string) {
  return {
    change: (change: SourceChange): Promise<{ state: SourceState }> =>
      request({ method: "changeProjectSource", projectId, change }),
    commit: (message: string | null, push: boolean, amend: boolean): Promise<{ outcome: CommitOutcome }> =>
      request({ method: "commitProjectChanges", projectId, message, includeUnstaged: false, push, amend }),
  };
}

/** The session a target belongs to, if it has started. */
export function sessionOf(target: SourceTarget): string | null {
  return target.type === "conversation" ? target.conversationId : null;
}

/** The checkout a target reads and changes. */
export function workspaceTargetOf(target: SourceTarget): WorkspaceTarget {
  return target.type === "conversation"
    ? { type: "conversation", conversationId: target.conversationId }
    : { type: "project", projectId: target.projectId };
}

/** Each target's unsent commit message, kept while the app runs: closing or switching the pane keeps it. */
export const unsent = new Map<string, string>();

/** The key a target's unsent message is kept under. */
export function targetKey(target: SourceTarget): string {
  return target.type === "conversation" ? target.conversationId : `project:${target.projectId}`;
}

export function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const CONFLICTS_LEFT = "There are merge conflicts. Resolve them before committing your changes.";

/** How the Commit button's menu commits: plain, amending the last commit, then pushing, or then pulling and pushing. */
export type CommitHow = "commit" | "amend" | "push" | "sync";

/** What a branch pick is for. */
export type Picking = "merge" | "checkout" | "rebase" | "from" | "delete";

/**
 * Whose changes each session's Source shows, by session: a worker's (its task id), read only,
 * or without one the session's own checkout. Kept while the app runs.
 */
export const useSourceScopes = create<Record<string, string>>(() => ({}));

/** Shows the session's own checkout in its Source (`taskId` null), or a worker's changes. */
export function setSourceScope(conversationId: string, taskId: string | null): void {
  useSourceScopes.setState((scopes) => {
    const { [conversationId]: _shown, ...rest } = scopes;
    return taskId ? { ...rest, [conversationId]: taskId } : rest;
  }, true);
}

/** Opens the right sidebar's Source on a worker's changes: its +N −N, clicked. */
export function showWorkerSource(conversationId: string, taskId: string): void {
  setSourceScope(conversationId, taskId);
  openWorkspacePane(conversationId, "source");
}
