import { create } from "zustand";

import type { ReviewScope, SourceState } from "@/ipc/generated";

/** How the Review tab draws diffs (its toolbar toggles and "Review options" menu). */
export type ReviewOptions = {
  /** Load whole files, so unchanged lines can be opened ("Don't load full files" off). */
  wholeFiles: boolean;
  ignoreWhitespace: boolean;
  wordDiffs: boolean;
  wrap: boolean;
  split: boolean;
  /** The file list beside the diffs is hidden. */
  hideFiles: boolean;
  /** Markdown files show rendered, not as a diff ("Enable rich preview"). */
  richPreview: boolean;
};

type ReviewState = {
  /** The scope each conversation's Review tab shows. */
  scopes: Record<string, ReviewScope>;
  options: ReviewOptions;
  /** What the Source panel last read of each conversation's checkout: a new reading (a stage,
   * a discard, a commit) shows in the Review tab at once. */
  checkouts: Record<string, string>;
};

/** The Review tab's scope per conversation and its options, kept while the app runs. */
export const useReview = create<ReviewState>(() => ({
  scopes: {},
  options: {
    wholeFiles: true,
    ignoreWhitespace: false,
    wordDiffs: false,
    wrap: false,
    split: false,
    hideFiles: false,
    richPreview: false,
  },
  checkouts: {},
}));

/** "Last Turn" of the latest request that changed files, until the user picks another. */
export const LATEST_TURN: ReviewScope = { type: "lastTurn", requestId: null };

export function reviewScope(conversationId: string): ReviewScope {
  return useReview.getState().scopes[conversationId] ?? LATEST_TURN;
}

export function setReviewScope(conversationId: string, scope: ReviewScope): void {
  useReview.setState((state) => ({ scopes: { ...state.scopes, [conversationId]: scope } }));
}

export function setReviewOption<K extends keyof ReviewOptions>(key: K, value: ReviewOptions[K]): void {
  useReview.setState((state) => ({ options: { ...state.options, [key]: value } }));
}

/** Notes what the Source panel read of the checkout; the Review tab reloads when it changes. */
export function noteSourceState(conversationId: string, state: SourceState): void {
  const reading = JSON.stringify([state.staged, state.changes]);
  if (useReview.getState().checkouts[conversationId] === reading) return;
  useReview.setState((current) => ({
    checkouts: { ...current.checkouts, [conversationId]: reading },
  }));
}
