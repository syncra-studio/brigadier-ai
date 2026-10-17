import { useShallow } from "zustand/react/shallow";

import { leftWords } from "@/app/conversation/undoWords";
import { request } from "@/ipc/client";
import { type Board, useBoard } from "@/state/board";
import { toast } from "@/state/toasts";

/** The files whose agents' changes wait for the user's Keep or Undo, in path order. */
export function pendingPaths(board: Board | null, conversationId: string | null): string[] {
  if (!board || board.conversationId !== conversationId) return [];
  const paths = new Set<string>();
  for (const entry of Object.values(board.requests)) {
    for (const [path, state] of Object.entries(entry.review ?? {})) if (state === "pending") paths.add(path);
  }
  return [...paths].toSorted();
}

export function usePendingPaths(conversationId: string | null): string[] {
  return useBoard(useShallow((s) => pendingPaths(s.board, conversationId)));
}

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Keeps changes: every file's, `path`'s, or some of them (`hunks`). False when it was refused. */
export async function keepChanges(conversationId: string, path?: string, hunks?: string[]): Promise<boolean> {
  try {
    await request({ method: "keepChanges", conversationId, ...(path !== undefined && { path }), ...(hunks && { hunks }) });
    return true;
  } catch (error) {
    toast(`Couldn't keep the changes: ${reason(error)}`, { tone: "error" });
    return false;
  }
}

/** Undoes changes, picked as Keep picks them; says what it left as it was. False when it was refused. */
export async function undoChanges(conversationId: string, path?: string, hunks?: string[]): Promise<boolean> {
  try {
    const { outcome } = await request({ method: "undoPendingChanges", conversationId, ...(path !== undefined && { path }), ...(hunks && { hunks }) });
    if (outcome.held.length > 0) toast(`${outcome.restored.length > 0 ? "Changes reverted" : "Nothing was reverted"}${leftWords(outcome.held)}`);
    return true;
  } catch (error) {
    toast(`Couldn't undo the changes: ${reason(error)}`, { tone: "error" });
    return false;
  }
}
