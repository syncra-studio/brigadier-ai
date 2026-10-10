import { useCallback, useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import type {
  EnvironmentKind,
  EnvironmentRequest,
  ModelChoice,
  PermissionLevel,
  Project,
  RepoInfo,
  SetupRequest,
} from "@/ipc/generated";
import {
  nothingAvailable,
  resolveModel,
  resolvePermission,
  useAvailableModelGroups,
  useModelGroups,
} from "@/lib/setup";
import { type DraftTarget, getRepoInfo } from "@/state/actions";
import { type DraftSetup, emptyDraft, type Selection, useApp } from "@/state/store";

export type RepoState = { info: RepoInfo | null; error: string | null; loading: boolean };

/** Repository reads by path, shared by the pickers; refreshed each time a draft opens. */
const repoCache = new Map<string, RepoInfo>();

/** The composer's view of a repository: branches, the checked-out branch, dirty state. */
export function useRepoInfo(path: string | null): RepoState & { reload: () => void } {
  const [attempt, setAttempt] = useState(0);
  const key = path === null ? null : `${attempt}:${path}`;
  const [result, setResult] = useState<{
    key: string;
    info: RepoInfo | null;
    error: string | null;
  } | null>(null);
  useEffect(() => {
    if (path === null || key === null) return;
    let live = true;
    getRepoInfo(path)
      .then((info) => {
        repoCache.set(path, info);
        if (live) setResult({ key, info, error: null });
      })
      .catch((cause: unknown) => {
        if (live) {
          setResult({ key, info: null, error: cause instanceof Error ? cause.message : String(cause) });
        }
      });
    return () => {
      live = false;
    };
  }, [path, key]);
  const reload = useCallback(() => setAttempt((value) => value + 1), []);
  const current = result !== null && result.key === key ? result : null;
  const cached = path === null ? null : (repoCache.get(path) ?? null);
  return {
    info: current?.info ?? cached,
    error: current?.error ?? null,
    loading: key !== null && current === null,
    reload,
  };
}

/** The draft's choices for this project (choices made for another project don't carry over). */
export function useDraft(projectId: string | null): DraftSetup {
  return useApp(
    useShallow((s) => (s.draft.projectId === projectId ? s.draft : emptyDraft(projectId))),
  );
}

export function updateDraft(projectId: string | null, patch: Partial<DraftSetup>): void {
  useApp.setState((state) => ({
    draft: {
      ...(state.draft.projectId === projectId ? state.draft : emptyDraft(projectId)),
      ...patch,
      projectId,
    },
  }));
}

/** Everything the composer resolved for a draft, and what (if anything) blocks sending. */
export type ResolvedDraft = {
  kind: "chat" | "session";
  project: Project | null;
  repoPath: string | null;
  repo: RepoState & { reload: () => void };
  draft: DraftSetup;
  environment: EnvironmentKind;
  /** Local checkout: the branch commits land on (after "New branch…", the new one). */
  branch: string | null;
  /** New worktree: the base branch. */
  base: string | null;
  permission: PermissionLevel;
  model: ModelChoice;
  /** What the draft starts with when no model is picked: the picker's Default. */
  defaultModel: ModelChoice;
  groups: ModelGroup[];
  target: DraftTarget | null;
  /** Why the draft can't be sent yet, in words. */
  problem: string | null;
  /** No model is available: every agent is off or all its models are hidden on Providers. */
  noModel: boolean;
};

/** Resolves a draft's setup: the composer's choices, then the project's, then the defaults. */
export function useResolvedDraft(selection: Selection): ResolvedDraft {
  const projectId =
    selection.type === "draft" && selection.kind === "session" ? selection.projectId : null;
  const kind = projectId ? "session" : "chat";
  const project = useApp((s) => (projectId ? (s.projects[projectId] ?? null) : null));
  const settings = useApp((s) => s.settings);
  const draft = useDraft(projectId);
  const listed = useModelGroups();
  const groups = useAvailableModelGroups();
  const noModel = nothingAvailable(listed, groups);
  const repoPath = project?.repos[0]?.path ?? null;
  const repo = useRepoInfo(kind === "session" ? repoPath : null);

  const model = resolveModel(draft.model, kind, project, settings, groups);
  const defaultModel = resolveModel(null, kind, project, settings, groups);
  const permission = resolvePermission(draft.permission, settings);
  const environment = draft.environment ?? project?.prefs.environment ?? "localCheckout";
  const current = repo.info?.currentBranch ?? null;
  const picked = draft.branch ?? current;
  const branch = draft.newBranch ?? picked;
  const base = draft.base ?? current;

  let problem: string | null = null;
  let target: DraftTarget | null = null;
  if (noModel) {
    problem = "No model is available. Turn one on in Providers.";
  } else if (kind === "chat") {
    target = { kind: "chat", setup: { type: "chat", model } };
  } else if (!project || !repoPath) {
    problem = "This project has no repository yet. Add one in its settings.";
  } else {
    let request: EnvironmentRequest | null = null;
    if (environment === "localCheckout") {
      if (!picked) {
        problem = repo.loading ? "Reading the repository…" : "Pick the branch commits land on.";
      } else {
        request = draft.newBranch
          ? { type: "localCheckout", branch: draft.newBranch, createFrom: picked }
          : { type: "localCheckout", branch: picked, createFrom: null };
      }
    } else if (!base) {
      problem = repo.loading ? "Reading the repository…" : "Pick the base branch.";
    } else {
      request = { type: "newWorktree", base, branch: draft.sessionBranch.trim() || null };
    }
    if (request) {
      const setup: SetupRequest = {
        type: "session",
        repo: repoPath,
        environment: request,
        permission,
        orchestrator: model,
        planMode: draft.planMode,
      };
      target = { kind: "session", projectId: project.id, setup };
    }
  }

  return {
    kind,
    project,
    repoPath,
    repo,
    draft,
    environment,
    branch,
    base,
    permission,
    model,
    defaultModel,
    groups,
    target,
    problem,
    noModel,
  };
}
