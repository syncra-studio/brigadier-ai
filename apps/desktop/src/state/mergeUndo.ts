import { useEffect, useState } from "react";

import { request } from "@/ipc/client";
import type { MergeUndo } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useSettledTabs } from "@/state/tabStates";

/** What became of a merge since, as git shows it ({@link useMergeUndo}). */
export type MergeSince = { undo: MergeUndo | null; pushed: boolean };

const UNKNOWN: MergeSince = { undo: null, pushed: false };

/**
 * What became of a session's merge on its base since it merged, as git shows it: when it was
 * reverted ("Undone on main · ‹time›") and redone after that (null while unknown or never
 * reverted), and whether its base's upstream has it now (a merge made on this Mac, pushed
 * since). Asked again when the session's merges or settled state change: a revert or redo the
 * thread makes lands as a merge of its own, one made by hand shows in the settled state. Until
 * it reads pushed, each step of the thread and each change of its turn asks again too: a push
 * is a command like any other.
 */
export function useMergeUndo(conversationId: string, mergeCommit: string | null | undefined): MergeSince {
  const merges = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.orchestratorSteps.filter((step) => step.kind.type === "merged").length : 0,
  );
  const settled = useSettledTabs((s) => s.byTab[conversationId]?.state ?? null);
  const [found, setFound] = useState<{ key: string; asked: string; since: MergeSince } | null>(null);
  const key = mergeCommit ? `${conversationId} ${mergeCommit}` : null;
  const pushed = !!found && found.key === key && found.since.pushed;
  const moves = useBoard((s) =>
    !pushed && s.board?.conversationId === conversationId ? `${s.board.orchestratorSteps.length} ${s.board.run}` : "",
  );
  // What it was asked after: a new merge or settled state asks again, and before it reads
  // pushed, the thread's moves.
  const asked = `${merges} ${settled ?? ""} ${moves}`;
  useEffect(() => {
    if (!key || !mergeCommit) return;
    let current = true;
    request({ method: "getMergeUndo", conversationId, mergeCommit })
      .then((answer) => {
        if (current) setFound({ key, asked, since: { undo: answer.undo ?? null, pushed: answer.pushed ?? false } });
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [key, conversationId, mergeCommit, asked]);
  return found && found.key === key ? found.since : UNKNOWN;
}
