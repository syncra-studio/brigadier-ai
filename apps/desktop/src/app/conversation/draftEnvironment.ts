import type { Conversation, EnvironmentKind, EnvironmentRequest } from "@/ipc/generated";

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
  /** The title of the session that already works in the project's own folder, if one does. */
  holder?: string | null;
};

/**
 * The session that already works in the repository's own folder: one not archived on a local
 * checkout there. A second one there would edit, commit and switch branches over it.
 */
export function checkoutHolder(conversations: readonly Conversation[], repo: string): Conversation | null {
  return (
    conversations.find(
      (other) =>
        other.kind === "session" &&
        other.lifecycle !== "archived" &&
        other.setup?.type === "session" &&
        other.setup.repo === repo &&
        other.setup.environment.type === "localCheckout",
    ) ?? null
  );
}

const READING = { problem: "Reading the repository…", waiting: true } as const;

/**
 * Where a draft session works, as the setup asks for it, or why it can't start yet. A new
 * worktree starts from its base; the local checkout works in the project's own folder, on the
 * branch it has checked out. `waiting`: it only waits for the repository to be read.
 */
export function environmentRequest(place: DraftPlace): { request: EnvironmentRequest } | { problem: string; waiting?: true } {
  const { environment, current, loading } = place;
  if (environment === "localCheckout") {
    if (!current) {
      return loading ? READING : { problem: "Your checkout isn't on a branch. Check one out to work there." };
    }
    if (place.holder) {
      return { problem: `“${place.holder}” already works in this checkout. Archive it, or start this session in a new worktree.` };
    }
    return { request: { type: "localCheckout", branch: current, createFrom: null } };
  }
  const base = place.base ?? current;
  if (!base) return loading ? READING : { problem: "Pick the base branch." };
  return { request: { type: "newWorktree", base, branch: place.sessionBranch.trim() || null } };
}
