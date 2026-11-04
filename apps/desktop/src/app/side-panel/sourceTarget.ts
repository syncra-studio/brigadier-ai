import { create } from "zustand";
import { persist } from "zustand/middleware";

import { request } from "@/ipc/client";
import type { CommitOutcome, SourceChange, SourceState } from "@/ipc/generated";
import { openWorkspacePane } from "@/state/workspacePane";
import type { WorkspaceTarget } from "@/state/workspaceCache";

/** Whose changes the panel shows: a session's checkout, a project's folder, or a worktree the user made. */
export type SourceTarget = WorkspaceTarget;

type SourceCalls = {
  change: (change: SourceChange) => Promise<{ state: SourceState }>;
  commit: (message: string | null, push: boolean, amend: boolean, includeUnstaged: boolean) => Promise<{ outcome: CommitOutcome }>;
};

/** The panel's changes, sent for its target: each kind has its own calls, so none falls through to another's checkout. */
export function sourceCalls(target: SourceTarget): SourceCalls {
  switch (target.type) {
    case "conversation":
      return sessionCalls(target.conversationId);
    case "project":
      return projectCalls(target.projectId);
    case "worktree":
      return worktreeCalls(target.projectId, target.path);
  }
}

function sessionCalls(conversationId: string): SourceCalls {
  return {
    change: (change) =>
      change.type === "stage"
        ? request({ method: "stageFiles", conversationId, paths: change.paths })
        : change.type === "unstage"
          ? request({ method: "unstageFiles", conversationId, paths: change.paths })
          : request({ method: "discardFiles", conversationId, scope: change.scope, paths: change.paths }),
    commit: (message, push, amend, includeUnstaged) =>
      request({ method: "commitChanges", conversationId, message, includeUnstaged, push, amend }),
  };
}

function projectCalls(projectId: string): SourceCalls {
  return {
    change: (change) => request({ method: "changeProjectSource", projectId, change }),
    commit: (message, push, amend, includeUnstaged) =>
      request({ method: "commitProjectChanges", projectId, message, includeUnstaged, push, amend }),
  };
}

function worktreeCalls(projectId: string, path: string): SourceCalls {
  return {
    change: (change) => request({ method: "changeWorktreeSource", projectId, path, change }),
    commit: (message, push, amend, includeUnstaged) =>
      request({ method: "commitWorktreeChanges", projectId, path, message, includeUnstaged, push, amend }),
  };
}

/** The session a target belongs to, if it has started. */
export function sessionOf(target: SourceTarget): string | null {
  return target.type === "conversation" ? target.conversationId : null;
}

/** Each target's unsent commit message, kept while the app runs: closing or switching the pane keeps it. */
export const unsent = new Map<string, string>();

/** The key a target's unsent message is kept under. */
export function targetKey(target: SourceTarget): string {
  switch (target.type) {
    case "conversation":
      return target.conversationId;
    case "project":
      return `project:${target.projectId}`;
    case "worktree":
      return `worktree:${target.projectId}:${target.path}`;
  }
}

export function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const CONFLICTS_LEFT = "There are merge conflicts. Resolve them before committing your changes.";

/** How the Commit button's menu commits: plain, amending the last commit, then pushing, or then pulling and pushing. */
export type CommitHow = "commit" | "amend" | "push" | "sync";

/** What a branch pick is for. */
export type Picking = "merge" | "checkout" | "rebase" | "from" | "delete";

/** What Commit does with changes but nothing staged: asks, stages them all, or stays off. */
export type StageAllChoice = "ask" | "always" | "never";

/** The answer "Always" or "Never" left, for every checkout and across launches. */
export const useStageAll = create<{ choice: StageAllChoice }>()(
  persist(() => ({ choice: "ask" as StageAllChoice }), { name: "brigadier.commitStageAll", version: 1 }),
);

export function setStageAll(choice: StageAllChoice): void {
  useStageAll.setState({ choice });
}

/** Changes, none staged, nothing under way: what Commit's "stage all" is for. */
export function nothingStaged(state: SourceState | null): boolean {
  return !!state && state.staged.length === 0 && state.changes.length > 0 && !state.merging && !state.rebasing;
}

/** What the main button does. */
export type MainAction = { kind: "commit" | "continue" | "publish" | "sync"; enabled: boolean };

/**
 * The main button, as Cursor's: Commit, or Continue during a merge or rebase, while there is
 * something to commit. Then Publish Branch for a branch with no upstream and a remote to publish
 * to, Sync Changes with commits to pull or push, and last a disabled Commit.
 */
export function mainAction(state: SourceState | null, choice: StageAllChoice): MainAction {
  if (!state) return { kind: "commit", enabled: false };
  if (state.merging || state.rebasing) {
    return { kind: "continue", enabled: !state.changes.some((file) => file.status === "conflicted") };
  }
  if (state.staged.length > 0) return { kind: "commit", enabled: true };
  if (state.changes.length > 0) return { kind: "commit", enabled: choice !== "never" };
  if (state.branch && !state.upstream && state.remote) return { kind: "publish", enabled: true };
  if (state.upstream && state.ahead + state.behind > 0) return { kind: "sync", enabled: true };
  return { kind: "commit", enabled: false };
}

/** Which of a Source's stacked rows are open, by row key; kept while the app runs. Unset rows use their default. */
const useOpenRows = create<Record<string, boolean>>(() => ({}));

export function useRowOpen(key: string, initial: boolean): [boolean, (open: boolean) => void] {
  const open = useOpenRows((rows) => rows[key] ?? initial);
  return [open, (next) => useOpenRows.setState({ [key]: next })];
}

/** The key a worker's row in its session's Source goes by. */
export const workerRowKey = (conversationId: string, taskId: string) => `session:${conversationId}:${taskId}`;

/**
 * The worker whose row each session's Source was just asked to open and bring into view (its
 * +N −N, clicked), by session.
 */
export const useSourceScopes = create<Record<string, string>>(() => ({}));

/** Asks a session's Source to open a worker's row (`taskId` null: nothing asked any more). */
export function setSourceScope(conversationId: string, taskId: string | null): void {
  useSourceScopes.setState((scopes) => {
    const { [conversationId]: _shown, ...rest } = scopes;
    return taskId ? { ...rest, [conversationId]: taskId } : rest;
  }, true);
}

/** Opens the right sidebar's Source on a worker's changes: its +N −N, clicked. */
export function showWorkerSource(conversationId: string, taskId: string): void {
  useOpenRows.setState({ [workerRowKey(conversationId, taskId)]: true });
  setSourceScope(conversationId, taskId);
  openWorkspacePane(conversationId, "source");
}
