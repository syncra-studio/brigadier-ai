import { create } from "zustand";

import { useBoard } from "@/state/board";

import { setPinnedSummary } from "@/state/actions";

export const useSummary = create<{
  layout: "beside" | "shift" | "float";
  floating: boolean;
}>(() => ({ layout: "beside", floating: false }));

/** Where each summary card, and their column, was scrolled to: `<session>/<card>` → offset. */
export const keptScroll = new Map<string, number>();

/** Open the same card beside the thread or in the narrow window's summary. */
export function revealPlan(cardId: string): void {
  reveal(() => {
    const run = Object.values(useBoard.getState().board?.overnight ?? {}).find(
      (candidate) => candidate.state !== "superseded" && (candidate.planId === cardId || candidate.planning?.planId === cardId),
    );
    return run ? `overnight-${run.id}` : `plan-${cardId}`;
  });
}

/** Open an overnight run's card, where its Start, Stop, Merge and Continue are. */
export function revealOvernight(runId: string): void {
  reveal(() => `overnight-${runId}`);
}

function reveal(elementId: () => string): void {
  const id = elementId();
  // The card opens at its top, wherever it and the column were left.
  for (const key of keptScroll.keys())
    if (key.endsWith(`/${id}`) || key.endsWith("/column")) keptScroll.delete(key);
  if (useSummary.getState().layout === "float")
    useSummary.setState({ floating: true });
  else setPinnedSummary(true);
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      const card = document.getElementById(id);
      const own = card?.closest<HTMLElement>("[data-slot=summary-card]");
      own?.firstElementChild?.scrollTo({ top: 0 });
      (own ?? card)?.scrollIntoView({ block: "nearest" });
      card?.focus({ preventScroll: true });
    }),
  );
}
