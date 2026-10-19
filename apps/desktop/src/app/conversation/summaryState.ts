import { create } from "zustand";

import { jumpInThread } from "@/components/assistant-ui/thread-scroll";
import type { EventEnvelope } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { useBoard } from "@/state/board";

import { setPinnedSummary } from "@/state/actions";
import { openPanelTab, openPlanTab } from "@/state/sessionTabs";

export const useSummary = create<{
  layout: "beside" | "shift" | "float";
  /** The layout is the thread pane's own, not the default before it is measured. */
  measured: boolean;
  floating: boolean;
}>(() => ({ layout: "beside", measured: false, floating: false }));

/** Where each summary card, and their column, was scrolled to: `<session>/<card>` → offset. */
export const keptScroll = new Map<string, number>();

/**
 * Open a plan: an overnight run's card beside the thread or in the narrow window's summary, a
 * plan written as a document in its own tab, a plan of phases alone in the job's tab.
 */
export function revealPlan(cardId: string): void {
  const board = useBoard.getState().board;
  const run = Object.values(board?.overnight ?? {}).find(
    (candidate) => candidate.state !== "superseded" && candidate.planId === cardId,
  );
  if (run) return reveal(() => `overnight-${run.id}`);
  const plan = board?.plans[cardId];
  if (!board || !plan) return;
  if (plan.body) openPlanTab(board.conversationId, { type: "plan", id: plan.id }, plan.title);
  else openPanelTab(board.conversationId, { type: "reviews" }, "Progress");
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
  if (useSummary.getState().layout === "float") useSummary.setState({ floating: true });
  else setPinnedSummary(true);
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      const target = document.getElementById(id);
      if (!target) return;
      // Its section may be folded.
      const section = target.closest<HTMLElement>("[data-slot=summary-section][data-folded]");
      section?.querySelector<HTMLButtonElement>(":scope > header > button[aria-expanded]")?.click();
      jumpInThread(target);
      // Its top, in its own card; then that card, in the column.
      const scroller = target.closest("[data-slot=summary-card]")?.firstElementChild;
      if (scroller && target.offsetHeight > scroller.clientHeight)
        scroller.scrollTop += target.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      target.scrollIntoView({ block: "nearest" });
      target.focus({ preventScroll: true });
    }),
  );
}

/**
 * Opens a conversation's card by itself, for real content it has to show: only where the thread
 * keeps its own room beside the card, and only while the card was never left shown or hidden, so
 * one the user closed stays closed.
 */
export function openSummaryByItself(key: string): void {
  const { layout, measured } = useSummary.getState();
  if (!measured || layout === "float" || key in useApp.getState().summaryShown) return;
  setPinnedSummary(true, key);
}

/** Only live arrivals in the loaded, visible conversation reveal its context. */
export function revealNewPreviews(events: readonly EventEnvelope[]): void {
  const { board } = useBoard.getState();
  const { selection } = useApp.getState();
  if (!board?.loaded || selection.type !== "conversation" || selection.id !== board.conversationId) return;
  if (!events.some(({ stream, event }) => stream === `conversation:${board.conversationId}`
    && event.type === "previewUpdated" && event.preview.conversationId === board.conversationId
    && !board.previews[event.preview.id])) return;
  openSummaryByItself(board.conversationId);
}
