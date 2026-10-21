import { type FC, useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import {
  CI_LINGER_MS,
  ciWatches,
  type CiWatchView,
  ciWords,
  heldMerge,
  mergeWords,
  type SlotRow,
  slotRows,
} from "@/app/conversation/activity/slotView";
import { stripWords } from "@/app/conversation/activity/stripView";
import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { CiPill } from "@/app/conversation/CiPill";
import { FoldedCountBadge } from "@/app/conversation/FoldedCountBadge";
import { MergePill } from "@/app/conversation/MergePill";
import { StatusSlotToggle } from "@/app/conversation/StatusSlotToggle";
import { ComposerRailItem } from "@/components/conversation";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * The one slot above the composer for live work (SESSION-FIXES-PLAN.md Q8): the Workers strip,
 * the "Checking CI…" pill and the held merge's pill, newest last, at most one of each. In a
 * narrow composer, more than one row folds into a line that counts them.
 */
export const ComposerStatusSlot: FC<{ conversationId: string }> = ({ conversationId }) => {
  const strip = useWorkersStrip(conversationId);
  // A watch's result lingers a few seconds: the clock moves on when the first one is due to go.
  const [now, setNow] = useState(() => Date.now());
  const ciJson = useBoard((s) => {
    if (s.board?.conversationId !== conversationId) return null;
    const watches = ciWatches(s.board.orchestratorSteps, now);
    return watches.length > 0 ? JSON.stringify(watches) : null;
  });
  const watches = useMemo(() => (ciJson ? (JSON.parse(ciJson) as CiWatchView[]) : null), [ciJson]);
  const lingerEnd = Math.min(...(watches ?? []).map((watch) => (watch.endedAtMs === null ? Infinity : watch.endedAtMs + CI_LINGER_MS)));
  useEffect(() => {
    if (!Number.isFinite(lingerEnd)) return;
    const timer = window.setTimeout(() => setNow(Math.max(Date.now(), lingerEnd)), Math.max(0, lingerEnd - Date.now()));
    return () => window.clearTimeout(timer);
  }, [lingerEnd]);
  const [mergeText, mergeBranch, mergeBase, mergeSince, mergeAsking] = useBoard(
    useShallow((s) => {
      const board = s.board;
      if (board?.conversationId !== conversationId) return [null, "", "", 0, false];
      const merge = heldMerge(board.orchestratorSteps, board.tasks, Object.values(board.questions));
      return merge ? [mergeWords(merge), merge.branch, merge.base, merge.sinceMs, merge.asking] : [null, "", "", 0, false];
    }),
  );
  const stripSince = useBoard((s) => {
    const tasks = s.board?.tasks;
    return strip && tasks ? Math.min(...strip.rows.map((id) => tasks[id]?.createdAtMs ?? 0)) : 0;
  });
  const [expanded, setExpanded] = useState(false);
  const rows = slotRows({
    ...(strip ? { workers: stripSince } : {}),
    ...(watches ? { ci: watches[0]?.sinceMs ?? 0 } : {}),
    ...(mergeText ? { merge: mergeSince } : {}),
  });
  if (rows.length === 0) return null;
  const words: Record<SlotRow, string> = {
    workers: strip ? stripWords(strip) : "",
    ci: watches ? ciWords(watches) : "",
    merge: mergeText ?? "",
  };
  const folds = rows.length > 1 && !expanded;
  return (
    <ComposerRailItem label="Live work">
      <div data-slot="composer-status-slot" className="@container/slot">
        {/* A narrow composer: one line, the newest row's words and how many more. */}
        {folds && (
          <div className="hidden px-1 py-1 @max-md/slot:flex">
            <StatusSlotToggle open={false} onToggle={() => setExpanded(true)} icon={<FoldedCountBadge count={rows.length} />}>
              {words[rows.at(-1)!]} · {rows.length - 1} more
            </StatusSlotToggle>
          </div>
        )}
        <div className={cn("flex flex-col divide-foreground/10 divide-y", folds && "@max-md/slot:hidden")}>
          {rows.map((row) =>
            row === "workers" && strip ? <WorkersStrip key={row} conversationId={conversationId} view={strip} />
            : row === "ci" && watches ? <CiPill key={row} watches={watches} />
            : row === "merge" && mergeText ? <MergePill key={row} conversationId={conversationId} words={mergeText} branch={mergeBranch} base={mergeBase} asking={mergeAsking} />
            : null,
          )}
        </div>
      </div>
    </ComposerRailItem>
  );
};
