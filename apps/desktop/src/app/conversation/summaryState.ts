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
  // The session's cards open where the plan is, not where they were left.
  const session = `${useBoard.getState().board?.conversationId}/`;
  for (const key of keptScroll.keys()) if (key.startsWith(session)) keptScroll.delete(key);
  if (useSummary.getState().layout === "float")
    useSummary.setState({ floating: true });
  else setPinnedSummary(true);
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      const target = document.getElementById(id);
      if (!target) return;
      // An earlier plan sits behind its disclosure.
      for (let fold = target.closest("details"); fold; fold = fold.parentElement?.closest("details") ?? null)
        fold.open = true;
      // Its top, in its own card; then that card, in the column.
      const scroller = target.closest("[data-slot=summary-card]")?.firstElementChild;
      if (scroller && target.offsetHeight > scroller.clientHeight)
        scroller.scrollTop += target.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      target.scrollIntoView({ block: "nearest" });
      target.focus({ preventScroll: true });
    }),
  );
}
