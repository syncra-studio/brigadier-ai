import { memo } from "react";
import { useShallow } from "zustand/react/shallow";

import { taskActivityLines, taskActivityTicks } from "@/app/conversation/taskActivity";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useActivityClock } from "@/hooks/use-activity-clock";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

export function useTaskActivityLine(taskId: string, withTime = true) {
  const active = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return !!task && taskActivityTicks(task);
  });
  const { ref, now } = useActivityClock(active);
  // Select only rendered primitives: unrelated workers and token ticks do not repaint this row.
  const lines = useBoard(useShallow((s) => {
    const board = s.board;
    const source = (id: string) => {
      const task = board?.tasks[id];
      const summary = board?.summaries[id];
      return {
        task,
        activity: board?.activity[id],
        diff: board?.diffs[id],
        summary: summary && summary.atMs >= (task?.attempts.at(-1)?.startedAtMs ?? task?.createdAtMs ?? 0)
          ? summary.text
          : undefined,
      };
    };
    return taskActivityLines(source(taskId), now, withTime);
  }));
  return { ref, ...lines };
}

/** Read once by assistive technology; token activity is deliberately not a live region. */
export const TaskActivity = memo(function TaskActivity({
  taskId,
  className,
}: { taskId: string; className?: string }) {
  const { ref, first, second, firstWorking, secondWorking } = useTaskActivityLine(taskId);
  if (!first) return null;
  return (
    <div ref={ref} data-slot="task-activity" className={cn("text-muted-foreground min-w-0 text-xs", className)}>
      {[first, second].filter(Boolean).map((text, index) => (
        <Tooltip key={index}>
          <TooltipTrigger asChild aria-describedby={undefined}>
            <div className="min-w-0 truncate">
              <span className={cn((index === 0 ? firstWorking : secondWorking) && "shimmer")}>{text}</span>
            </div>
          </TooltipTrigger>
          <TooltipContent aria-hidden>{text}</TooltipContent>
        </Tooltip>
      ))}
    </div>
  );
});
