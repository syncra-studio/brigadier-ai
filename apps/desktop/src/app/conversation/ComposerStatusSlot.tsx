import { Branch, ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import {
  type CiWatchView,
  ciWatches,
  ciWords,
  heldMerge,
  mergeWords,
  shortSha,
  type SlotRow,
  slotRows,
} from "@/app/conversation/activity/slotView";
import { stripWords } from "@/app/conversation/activity/stripView";
import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { useAction } from "@/app/conversation/useAction";
import { ComposerRailItem } from "@/components/assistant-ui/elements/composer-rail";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { openUrl } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { cancelMerge } from "@/state/actions";
import { useBoard } from "@/state/board";

/** A row's words and toggle, as the Workers strip draws its own: grey, its chevron after. */
const Toggle: FC<{ open: boolean; onToggle: () => void; icon: ReactNode; children: ReactNode }> = ({ open, onToggle, icon, children }) => (
  <button
    type="button"
    aria-expanded={open}
    onClick={onToggle}
    data-state={open ? "open" : "closed"}
    className="group/toggle hover:text-foreground text-foreground/60 rounded-control flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-start text-sm"
  >
    <span className="flex size-4 shrink-0 items-center justify-center">{icon}</span>
    <span className="flex min-w-0 items-center">
      <span className="min-w-0 truncate tabular-nums">{children}</span>
      <span aria-hidden className="size-icon-button-sm flex shrink-0 items-center justify-center">
        <ChevronRight className="text-row-chevron size-chevron group-hover/toggle:text-foreground group-focus-visible/toggle:text-foreground transition-[rotate,color] duration-300 ease-standard group-data-[state=open]/toggle:rotate-90 motion-reduce:transition-none" />
      </span>
    </span>
  </button>
);

/** "Checking CI · 2 of 5 jobs done": every push being watched, each run's jobs and link inside. */
const CiPill: FC<{ watches: readonly CiWatchView[] }> = ({ watches }) => {
  const [open, setOpen] = useState(false);
  return (
    <div data-slot="ci-pill" className="flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <Toggle open={open} onToggle={() => setOpen(!open)} icon={<Spinner className="size-icon-xs animate-spin motion-reduce:animate-none" />}>
          {ciWords(watches)}
        </Toggle>
      </div>
      {open && (
        <ul className="max-h-queue-max flex flex-col gap-1 overflow-y-auto px-2 pb-1">
          {watches.map((watch) => (
            <li key={`${watch.repo} ${watch.branch} ${watch.sha}`} className="flex flex-col">
              <span className="text-foreground/80 min-h-row-sm flex min-w-0 items-center gap-1.5 ps-6 text-sm">
                <span className="truncate">{watch.repo}</span>
                <span aria-hidden className="text-foreground/40">·</span>
                <span className="truncate">{watch.branch}</span>
                <span aria-hidden className="text-foreground/40">·</span>
                <span className="text-foreground/50 font-mono text-xs">{shortSha(watch.sha)}</span>
              </span>
              {watch.runs.length === 0 ? (
                <span className="text-foreground/50 ps-6 text-sm">Waiting for CI to start…</span>
              ) : (
                watch.runs.map((run) => (
                  <button
                    key={run.url}
                    type="button"
                    onClick={() => void openUrl(run.url).catch(() => {})}
                    className="hover:bg-foreground/5 rounded-control min-h-row-sm text-foreground/60 hover:text-foreground flex min-w-0 items-center gap-1.5 ps-6 pe-2 text-start text-sm"
                  >
                    <span className="min-w-0 flex-1 truncate">{run.workflow}</span>
                    <span className="shrink-0 tabular-nums">
                      {run.jobsTotal > 0 ? `${run.jobsDone} of ${run.jobsTotal} jobs done` : run.status === "queued" ? "Queued" : "Starting"}
                    </span>
                  </button>
                ))
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};

/** "Merging after 2 workers finish", with Cancel: the user's yes, held while workers write. */
const MergePill: FC<{ conversationId: string; words: string; branch: string; base: string }> = ({ conversationId, words, branch, base }) => {
  const cancel = useAction();
  return (
    <div data-slot="merge-pill" className="flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="text-foreground/60 flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-sm">
              <Branch aria-hidden className="size-4 shrink-0" />
              <span className="min-w-0 truncate">{words}</span>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top">
            Merges {branch} into {base} once they’re done, as you asked
          </TooltipContent>
        </Tooltip>
        <Button
          size="xs"
          variant="ghost"
          data-slot="merge-cancel"
          disabled={cancel.busy}
          onClick={() => cancel.run(() => cancelMerge(conversationId))}
          className="text-foreground/60 hover:text-foreground"
        >
          Cancel
        </Button>
      </div>
      {cancel.error && <p role="alert" className="text-destructive px-2 text-xs">{cancel.error}</p>}
    </div>
  );
};

/**
 * The one slot above the composer for live work (SESSION-FIXES-PLAN.md Q8): the Workers strip,
 * the "Checking CI…" pill and the held merge's pill, newest last, at most one of each. In a
 * narrow composer, more than one row folds into a line that counts them.
 */
export const ComposerStatusSlot: FC<{ conversationId: string }> = ({ conversationId }) => {
  const strip = useWorkersStrip(conversationId);
  const ciJson = useBoard((s) => {
    if (s.board?.conversationId !== conversationId) return null;
    const watches = ciWatches(s.board.orchestratorSteps);
    return watches.length > 0 ? JSON.stringify(watches) : null;
  });
  const watches = useMemo(() => (ciJson ? (JSON.parse(ciJson) as CiWatchView[]) : null), [ciJson]);
  const [mergeText, mergeBranch, mergeBase, mergeSince] = useBoard(
    useShallow((s) => {
      const board = s.board;
      if (board?.conversationId !== conversationId) return [null, "", "", 0];
      const merge = heldMerge(board.orchestratorSteps, board.tasks, Object.values(board.reviews));
      return merge ? [mergeWords(merge), merge.branch, merge.base, merge.sinceMs] : [null, "", "", 0];
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
            <Toggle open={false} onToggle={() => setExpanded(true)} icon={<span className="bg-foreground/10 text-foreground/70 rounded-full px-1.5 text-xs tabular-nums">{rows.length}</span>}>
              {words[rows.at(-1)!]} · {rows.length - 1} more
            </Toggle>
          </div>
        )}
        <div className={cn("flex flex-col divide-foreground/10 divide-y", folds && "@max-md/slot:hidden")}>
          {rows.map((row) =>
            row === "workers" && strip ? <WorkersStrip key={row} conversationId={conversationId} view={strip} />
            : row === "ci" && watches ? <CiPill key={row} watches={watches} />
            : row === "merge" && mergeText ? <MergePill key={row} conversationId={conversationId} words={mergeText} branch={mergeBranch} base={mergeBase} />
            : null,
          )}
        </div>
      </div>
    </ComposerRailItem>
  );
};
