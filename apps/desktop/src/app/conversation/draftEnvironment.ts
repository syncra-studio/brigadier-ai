import type { EnvironmentKind, EnvironmentRequest } from "@/ipc/generated";

/** What the composer knows when it asks where a new session works. */
export type DraftPlace = {
  environment: EnvironmentKind;
  /** New worktree: the base picked in the branch pill (absent: the checked-out branch). */
  base: string | null;
  /** New worktree: the session branch's name (empty: Brigadier names it). */
  sessionBranch: string;
  /** The branch the project's own folder has checked out; absent when detached or unread. */
  current: string | null;
  /** The repository is still being read. */
  loading: boolean;
};

/**
 * Where a draft session works, as the setup asks for it, or why it can't start yet. A new
 * worktree starts from its base; the local checkout works in the project's own folder, on the
 * branch it has checked out.
 */
export function environmentRequest(place: DraftPlace): { request: EnvironmentRequest } | { problem: string } {
  const { environment, current, loading } = place;
  if (environment === "localCheckout") {
    if (!current) {
      return { problem: loading ? "Reading the repository…" : "Your checkout isn't on a branch. Check one out to work there." };
    }
    return { request: { type: "localCheckout", branch: current, createFrom: null } };
  }
  const base = place.base ?? current;
  if (!base) return { problem: loading ? "Reading the repository…" : "Pick the base branch." };
  return { request: { type: "newWorktree", base, branch: place.sessionBranch.trim() || null } };
}
