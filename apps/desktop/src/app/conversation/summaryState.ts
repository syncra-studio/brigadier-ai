import { create } from "zustand";

import type { EventEnvelope } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { useBoard } from "@/state/board";

import { setPinnedSummary } from "@/state/actions";

export const useSummary = create<{
  layout: "beside" | "shift" | "float";
  /** The layout is the thread pane's own, not the default before it is measured. */
  measured: boolean;
  floating: boolean;
}>(() => ({ layout: "beside", measured: false, floating: false }));

/** Where each summary card, and their column, was scrolled to: `<session>/<card>` → offset. */
export const keptScroll = new Map<string, number>();

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
