import { create } from "zustand";

import type { SourceState } from "@/ipc/generated";

/** How the Review tab draws diffs (its toolbar toggles). */
export type ReviewOptions = {
  wrap: boolean;
  split: boolean;
};

type ReviewState = {
  options: ReviewOptions;
  /** What the Source panel last read of each conversation's checkout: a new reading (a stage,
   * a discard, a commit) shows in the Review tab at once. */
  checkouts: Record<string, string>;
};

/** The Review tab's options, kept while the app runs. */
export const useReview = create<ReviewState>(() => ({
  options: { wrap: false, split: false },
  checkouts: {},
}));

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
