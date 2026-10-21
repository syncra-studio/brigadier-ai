import { useShallow } from "zustand/react/shallow";

import { taskActivityLines, taskActivityTicks } from "@/app/conversation/taskActivity";
import { useActivityClock } from "@/hooks/use-activity-clock";
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
        note: board?.notes?.[id],
        summary: summary && summary.atMs >= (task?.attempts.at(-1)?.startedAtMs ?? task?.createdAtMs ?? 0)
          ? summary.text
          : undefined,
      };
    };
    return taskActivityLines(source(taskId), now, withTime);
  }));
  return { ref, ...lines };
}
