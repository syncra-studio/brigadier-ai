import { useMemo } from "react";

import { type BlockState, isFinal } from "@/app/conversation/blocks";
import type { StatusHead } from "@/app/conversation/liveStatus";
import { heldWait, liveWait } from "@/app/conversation/waitWords";
import type { WorkSpan } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useCliThread, useTurnCommands, useTurnSubagents, useTurnWorkers } from "@/state/cliThread";
import { backgroundOf } from "@/state/nativeWorkers";

type HeldBlock = { state: BlockState; startedAtMs: number; endedAtMs: number | null; worked: readonly WorkSpan[]; requestIds: readonly string[] };

const NO_STARTS: readonly number[] = [];

/**
 * What a block's turns started that still works past them, and so holds it open (D24): its
 * workers, its background commands, its CLI's own subagents. One reading for its header and for
 * the composer, so the two say the same (D94).
 */
export function useBlockHold(conversationId: string | null, meta: HeldBlock | null | undefined) {
  const requestIds = meta?.requestIds;
  const workersActive = useBoard((s) =>
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId !== null && !!requestIds?.includes(task.requestId) && !isFinal(task),
    ),
  );
  // The workers its turn started that still work: over, it stays live until they finish (its own
  // wake-ups when they do are in the same turn).
  const starts = meta?.state === "done" ? meta.worked.map((span) => span.fromMs) : NO_STARTS;
  const workers = useTurnWorkers(conversationId, starts, meta?.endedAtMs ?? null);
  // So do the background commands it started (a CI watch, a Monitor) until the turn their end
  // starts is over (D21), though its work folds as it would without them.
  const commands = useTurnCommands(conversationId, starts, meta?.endedAtMs ?? null);
  // Its CLI's own subagents that still work, foreground or in the background, hold it too (D24, D97).
  const subagents = useTurnSubagents(conversationId, meta?.startedAtMs ?? Infinity, meta?.endedAtMs ?? null);
  // The last of its background subagents to end: an end no turn of its follows (Codex's) still
  // ends its work there, so the time it counted while held stays (D53). Other ends start a turn.
  const heldUntil = useCliThread((s) => {
    const view = conversationId && meta?.endedAtMs != null ? s.views[conversationId] : undefined;
    const ends = (view ? backgroundOf(view) : [])
      .filter((task) => task.kind === "agent" && task.startedAtMs >= meta!.startedAtMs && task.startedAtMs <= meta!.endedAtMs!)
      .flatMap((task) => (task.endedAtMs != null ? [task.endedAtMs] : []));
    return ends.length > 0 ? Math.max(...ends) : null;
  });
  // What holds it, named when it is one thing (D106); a string, so it stays the same while that does.
  const named = useCliThread((s) => {
    const view = conversationId && meta ? s.views[conversationId] : undefined;
    return view ? JSON.stringify(heldWait(view, starts, meta!.startedAtMs, meta!.endedAtMs)) : "null";
  });
  const held = workers > 0 || commands > 0 || workersActive || subagents > 0;
  return { workers, commands, subagents, held, heldUntil, named: useMemo(() => JSON.parse(named) as StatusHead | null, [named]) };
}

/** What the open turn of a session's CLI is blocked on now, named (`liveWait`); `null` when nothing. */
export function useLiveWait(conversationId: string | null): StatusHead | null {
  const json = useCliThread((s) => {
    const view = conversationId ? s.views[conversationId] : undefined;
    return view ? JSON.stringify(liveWait(view)) : "null";
  });
  return useMemo(() => JSON.parse(json) as StatusHead | null, [json]);
}
