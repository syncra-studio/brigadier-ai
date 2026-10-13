import { create } from "zustand";

import type { EventEnvelope } from "@/ipc/generated";
import { CHAT_TAB, sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { useBoard } from "@/state/board";

import { setPinnedSummary } from "@/state/actions";

export const useSummary = create<{
  layout: "beside" | "shift" | "float";
  floating: boolean;
  /** The next floating open keeps focus in the composer. */
  automatic: boolean;
  /** A plan the thread asked to see; the Plan section shows it until another plan takes over. */
  plan: string | null;
}>(() => ({ layout: "beside", floating: false, automatic: false, plan: null }));

/** Where each summary card, and their column, was scrolled to: `<session>/<card>` → offset. */
export const keptScroll = new Map<string, number>();

/** Open the same card beside the thread or in the narrow window's summary. */
export function revealPlan(cardId: string): void {
  reveal(() => {
    const run = Object.values(useBoard.getState().board?.overnight ?? {}).find(
      (candidate) => candidate.state !== "superseded" && candidate.planId === cardId,
    );
    if (run) return `overnight-${run.id}`;
    // The Plan section shows one plan: an earlier one takes its place until another takes over.
    useSummary.setState({ plan: cardId });
    return `plan-${cardId}`;
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
    useSummary.setState({ floating: true, automatic: false });
  else setPinnedSummary(true);
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      const target = document.getElementById(id);
      if (!target) return;
      // The Plan section may be folded.
      const section = target.closest<HTMLElement>("[data-slot=summary-section][data-folded]");
      section?.querySelector<HTMLButtonElement>(":scope > header > button[aria-expanded]")?.click();
      // Its top, in its own card; then that card, in the column.
      const scroller = target.closest("[data-slot=summary-card]")?.firstElementChild;
      if (scroller && target.offsetHeight > scroller.clientHeight)
        scroller.scrollTop += target.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      target.scrollIntoView({ block: "nearest" });
      target.focus({ preventScroll: true });
    }),
  );
}

/** Only live arrivals in the loaded, visible conversation reveal its context. */
export function revealNewPreviews(events: readonly EventEnvelope[]): void {
  const { board } = useBoard.getState();
  const { selection } = useApp.getState();
  if (!board?.loaded || selection.type !== "conversation" || selection.id !== board.conversationId) return;
  if (!events.some(({ stream, event }) => stream === `conversation:${board.conversationId}`
    && event.type === "previewUpdated" && event.preview.conversationId === board.conversationId
    && !board.previews[event.preview.id])) return;
  if (useSummary.getState().layout === "float") {
    if (sessionTabs(board.conversationId).active !== CHAT_TAB) return;
    if (!useSummary.getState().floating) useSummary.setState({ floating: true, automatic: true });
  } else useApp.setState({ pinnedSummary: true });
  // The section may have been folded in this or another session. Wait for the arrival to render.
  requestAnimationFrame(() => {
    document.querySelectorAll<HTMLElement>('[data-activities][data-folded] > header > button[aria-expanded="false"]')
      .forEach((button) => button.click());
  });
}
