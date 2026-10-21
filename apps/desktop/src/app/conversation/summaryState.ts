import { create } from "zustand";

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
 * Open a plan: an overnight run's part of the job's tab, a plan written as a document in its own
 * tab, a plan of phases alone in the job's tab.
 */
export function revealPlan(cardId: string): void {
  const board = useBoard.getState().board;
  const run = Object.values(board?.overnight ?? {}).find(
    (candidate) => candidate.state !== "superseded" && candidate.planId === cardId,
  );
  if (run) return revealOvernight(run.id);
  const plan = board?.plans[cardId];
  if (!board || !plan) return;
  if (plan.body) openPlanTab(board.conversationId, { type: "plan", id: plan.id }, plan.title);
  else openPanelTab(board.conversationId, { type: "reviews" }, "Progress");
}

/**
 * Open an overnight run's plan (the Job card's Edit, a plan link in the thread): the run's part
 * of the job's tab, each phase with its scope and "done when", scrolled to and focused.
 */
export function revealOvernight(runId: string): void {
  const board = useBoard.getState().board;
  if (!board) return;
  openPanelTab(board.conversationId, { type: "reviews" }, "Progress");
  // The tab loads on demand: wait (a second at most) for the run's part to be drawn.
  let frames = 0;
  const find = () => {
    const target = document.getElementById(`overnight-${runId}`);
    if (target) {
      target.scrollIntoView({ block: "start" });
      target.focus({ preventScroll: true });
    } else if (++frames < 60) requestAnimationFrame(find);
  };
  requestAnimationFrame(find);
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
