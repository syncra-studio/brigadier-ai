import { useEffect, useState } from "react";

import { request } from "@/ipc/client";
import type { MergeUndo } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useSettledTabs } from "@/state/tabStates";

/**
 * What became of a session's merge on its base since it merged, as git shows it: when it was
 * reverted ("Undone on main · ‹time›") and redone after that. Null while unknown or never
 * reverted. Asked again when the session's merges or settled state change: a revert or redo the
 * thread makes lands as a merge of its own, one made by hand shows in the settled state.
 */
export function useMergeUndo(conversationId: string, mergeCommit: string | null | undefined): MergeUndo | null {
  const merges = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.orchestratorSteps.filter((step) => step.kind.type === "merged").length : 0,
  );
  const settled = useSettledTabs((s) => s.byTab[conversationId]?.state ?? null);
  const [found, setFound] = useState<{ key: string; undo: MergeUndo | null } | null>(null);
  const key = mergeCommit ? `${conversationId} ${mergeCommit}` : null;
  useEffect(() => {
    if (!key || !mergeCommit) return;
    let current = true;
    request({ method: "getMergeUndo", conversationId, mergeCommit })
      .then(({ undo }) => {
        if (current) setFound({ key, undo: undo ?? null });
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [key, conversationId, mergeCommit, merges, settled]);
  return found && found.key === key ? found.undo : null;
}
