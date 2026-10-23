import type { FC } from "react";
import { useShallow } from "zustand/react/shallow";

import { Capsule } from "@/app/conversation/Capsule";
import { DiffPill } from "@/app/conversation/DiffPill";
import { mergeStats } from "@/app/conversation/mergeStats";
import type { DiffStat, UserRequest } from "@/ipc/generated";
import { openFileTab } from "@/state/sessionTabs";
import { useBoard } from "@/state/board";

/**
 * What a request changed so far, with the messages steered into it: what each recorded of its
 * own changes (`changes`: the thread's own edits and commands and its workers' landings, never
 * the user's), else, for a request from before that was kept, the stats of its workers' landed
 * commits. It keeps counting after the orchestrator's reply ended (the turn's diff reads it too),
 * so it is tied to the request, not to the turn. Nothing changed yet: `null`, never "0 files".
 */
export function useRequestDiff(requestId: string | null): DiffStat | null {
  const stats = useBoard(
    useShallow((s) => {
      if (requestId === null || !s.board) return [];
      const { requests, tasks } = s.board;
      const ids = Object.values(requests)
        .filter((request) => request.id === requestId || request.steeredInto === requestId)
        .map((request) => request.id);
      return ids.flatMap((id) => {
        const recorded = requests[id]?.changes;
        if (recorded) return [recorded.stat];
        return Object.values(tasks)
          .filter((task) => task.requestId === id && task.landed !== null)
          .toSorted((a, b) => a.number - b.number)
          .flatMap((task) => (task.candidate ? [task.candidate.diffStat] : []));
      });
    }),
  );
  const merged = mergeStats(stats);
  return merged && merged.files.length > 0 ? merged : null;
}

/** The newest non-steered request that is still working owns the surface. */
export function activeRequest(requests: Readonly<Record<string, UserRequest>>): UserRequest | null {
  return (
    Object.values(requests)
      .filter((request) => request.steeredInto === null && request.state.type === "working")
      .toSorted((a, b) => b.startedAtMs - a.startedAtMs)[0] ?? null
  );
}

/** A request's capsule: what its workers landed. */
export const RequestCapsule: FC<{ conversationId: string }> = ({ conversationId }) => {
  const requestId = useBoard((s) =>
    s.board?.conversationId === conversationId ? activeRequest(s.board.requests)?.id ?? null : null,
  );
  const diff = useRequestDiff(requestId);
  if (!requestId || !diff) return null;
  return (
    <Capsule>
      <DiffPill
        diff={diff}
        {...(diff.files[0] && { onOpen: () => openFileTab(conversationId, diff.files[0]!.path) })}
      />
    </Capsule>
  );
};
