import type { Conversation, EnvironmentRequest } from "@/ipc/generated";
import type { DraftEnvironment } from "@/state/store";

/** What the composer knows when it asks where a new session works. */
export type DraftPlace = {
  environment: DraftEnvironment;
  /** New worktree: the base picked in the branch pill (absent: the checked-out branch). */
  base: string | null;
  /** Existing worktree: the folder picked in the worktree pill. */
  worktree?: string | null;
  /** New worktree: the session branch's name (empty: Brigadier names it). */
  sessionBranch: string;
  /** The branch the project's own folder has checked out; absent when detached or unread. */
  current: string | null;
  /** The repository is still being read. */
  loading: boolean;
};

const READING = { problem: "Reading the repository…", waiting: true } as const;

/**
 * Where a draft session works, as the setup asks for it, or why it can't start yet. A new
 * worktree starts from its base; the local checkout works in the project's own folder, on the
 * branch it has checked out; an existing worktree is worked in on its branch, beside any
 * session already there. `waiting`: it only waits for the repository to be read.
 */
export function environmentRequest(place: DraftPlace): { request: EnvironmentRequest } | { problem: string; waiting?: true } {
  const { environment, current, loading } = place;
  if (environment === "existingWorktree") {
    if (place.worktree) return { request: { type: "existing", path: place.worktree } };
    return loading ? READING : { problem: "Pick the worktree to work in." };
  }
  if (environment === "localCheckout") {
    if (!current) {
      return loading ? READING : { problem: "Your checkout isn't on a branch. Check one out to work there." };
    }
    return { request: { type: "localCheckout", branch: current, createFrom: null } };
  }
  const base = place.base ?? current;
  if (!base) return loading ? READING : { problem: "Pick the base branch." };
  return { request: { type: "newWorktree", base, branch: place.sessionBranch.trim() || null } };
}

/** A path as both git and Brigadier may write it: macOS's /private/tmp is /tmp. */
const samePath = (path: string) => path.replace(/^\/private(?=\/)/, "");

/**
 * What a worktree is, in a word the user knows (D184): the sessions that work there by title,
 * else the folder's own name.
 */
export function worktreeLabel(path: string, conversations: Readonly<Record<string, Conversation>>): string {
  const titles = Object.values(conversations)
    .filter(({ setup }) => setup?.type === "session" && setup.environment.type === "newWorktree" && !!setup.environment.path && samePath(setup.environment.path) === samePath(path))
    .toSorted((a, b) => a.createdAtMs - b.createdAtMs)
    .map(({ title }) => title);
  return titles.length ? titles.join(", ") : (path.split("/").findLast(Boolean) ?? path);
}
