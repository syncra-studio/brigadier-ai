import { Agent, ChevronRight, Stop } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { isWorking } from "@/app/conversation/blocks";
import { useAction } from "@/app/conversation/useAction";
import { Changes, WorkerStripRow, workerStat } from "@/app/conversation/WorkerSummary";
import { ComposerRailItem } from "@/components/assistant-ui/elements/composer-rail";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { stopTask } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The "N background workers" strip on the composer: while any worker of the session is really
 * at work, a line with how many, the +N −N they haven't landed yet, Stop all and a chevron;
 * opened, those workers, one per row (glyph, name and what it is at, which opens it), and the
 * hint to tag them. Checkers count with what they check, and what landed or waits counts not.
 */
export const BackgroundWorkers: FC<{ conversationId: string }> = ({ conversationId }) => {
  const tasks = useBoard(
    useShallow((s) =>
      s.board?.conversationId === conversationId ? Object.values(s.board.tasks) : [],
    ),
  );
  const diffs = useBoard((s) => s.board?.diffs);
  const action = useAction();
  const [open, setOpen] = useState(false);
  const alive = tasks
    .filter((task) => isWorking(task) && task.gateLink === null)
    .toSorted((a, b) => a.number - b.number);
  if (alive.length === 0) return null;
  const stats = alive.flatMap((task) => workerStat(task, diffs) ?? []);
  const insertions = stats.reduce((sum, stat) => sum + stat.insertions, 0);
  const deletions = stats.reduce((sum, stat) => sum + stat.deletions, 0);
  const summary = `${alive.length} background ${alive.length === 1 ? "worker" : "workers"}`;

  return (
    <ComposerRailItem label="Background workers">
      <Collapsible
        open={open}
        onOpenChange={setOpen}
        data-slot="background-workers"
        className="px-2.5 py-0.5 text-sm"
      >
        <div className="flex min-w-0 items-center gap-2">
          <CollapsibleTrigger className="text-muted-foreground flex min-h-control-sm min-w-0 flex-1 items-center gap-2 text-start">
            <Agent aria-hidden className="text-muted-foreground/70 size-icon-xs shrink-0" />
            <span className="min-w-0 flex-1 truncate">
              {summary}
              {open && <span className="text-muted-foreground/70"> (@ to tag workers)</span>}
            </span>
          </CollapsibleTrigger>
          <div className="flex shrink-0 items-center gap-1">
            <Changes insertions={insertions} deletions={deletions} monochrome />
            <TooltipIconButton
              tooltip="Stop all workers in this session"
              side="top"
              size="icon-sm"
              className="text-muted-foreground hover:text-foreground"
              disabled={action.busy}
              onClick={() => action.run(() => Promise.all(alive.map((task) => stopTask(task.id))))}
            >
              <Stop className="size-icon-xs" />
            </TooltipIconButton>
            <TooltipIconButton
              tooltip={open ? "Hide background workers" : "Show background workers"}
              side="top"
              size="icon-sm"
              aria-expanded={open}
              className="text-muted-foreground hover:text-foreground"
              onClick={() => setOpen(!open)}
            >
              <ChevronRight
                className={cn(
                  "size-icon-xs transition-transform duration-300 motion-reduce:transition-none",
                  open && "rotate-90",
                )}
              />
            </TooltipIconButton>
          </div>
        </div>
        {action.error && (
          <p role="alert" className="text-destructive text-xs">
            {action.error}
          </p>
        )}
        <CollapsibleContent className="data-[state=open]:animate-collapsible-down data-[state=closed]:animate-collapsible-up flex flex-col overflow-hidden pb-1">
          {alive.map((task) => (
            <WorkerStripRow key={task.id} taskId={task.id} className="ps-5" />
          ))}
        </CollapsibleContent>
      </Collapsible>
    </ComposerRailItem>
  );
};
