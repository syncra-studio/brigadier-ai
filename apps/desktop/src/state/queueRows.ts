import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";

import type { MessageQueue, QueuedMessage } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useApp, type PendingMessage } from "@/state/store";

/** A row of the queue rail: one of the daemon's queue, or one sent that it doesn't have yet. */
export type QueueRow = QueuedMessage & { local?: true };

const NO_ROWS: QueueRow[] = [];
const NO_PENDING: PendingMessage[] = [];

/**
 * The queue rail's rows: the daemon's queue, with each message sent to wait in it shown in its
 * slot from the moment it is sent (#491). Its row gives way to the daemon's item once that
 * shows, and leaves when the daemon sent it on or refused it.
 */
export function queueRows(items: readonly QueuedMessage[], sent: readonly PendingMessage[]): QueueRow[] {
  const waiting = sent.filter((entry) => entry.queued);
  if (waiting.length === 0) return items as QueueRow[];
  const rows: QueueRow[] = [...items];
  for (const entry of waiting) {
    const { index, deciding } = entry.queued!;
    rows.splice(Math.min(index ?? rows.length, rows.length), 0, {
      id: entry.localId,
      text: entry.text,
      attachments: entry.attachments,
      mentions: [...(entry.mentions ?? [])],
      queuedAtMs: entry.createdAtMs,
      editedAtMs: null,
      deciding,
      local: true,
    });
  }
  return rows;
}

/** The conversation's queue as the rail shows it ([`queueRows`]), `null` before its board. */
export function useQueueRows(conversationId: string): { queue: MessageQueue | null; rows: QueueRow[] } {
  const queue = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.queue : null));
  const sent = useApp(
    useShallow((s) => {
      const waiting = s.pending.filter((entry) => entry.conversationId === conversationId && entry.queued);
      return waiting.length > 0 ? waiting : NO_PENDING;
    }),
  );
  const rows = useMemo(() => queueRows(queue?.items ?? NO_ROWS, sent), [queue, sent]);
  return { queue, rows };
}
