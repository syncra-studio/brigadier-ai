import type { FC } from "react";
import { useShallow } from "zustand/react/shallow";

import { QuietBrief } from "@/app/conversation/activity/QuietBrief";
import { WorkerName } from "@/app/conversation/activity/WorkerName";
import type { Task } from "@/ipc/generated";
import { type Board, useBoard } from "@/state/board";

/** What a worker's routing row picked, in its start's words (goal 12). */
export const pickedRow = (s: { board: Board | null }, taskId: string) => {
  const start = s.board?.orchestratorSteps.find((step) => step.kind.type === "created" && step.kind.taskId === taskId);
  return start?.kind.type === "created" ? start.kind.row : undefined;
};

/** A start's detail: each worker's brief, in a quiet box, and what its routing row picked (goal 12). */
export const Briefs: FC<{ tasks: readonly Task[] }> = ({ tasks }) => {
  const rows = useBoard(useShallow((s) => tasks.map((task) => pickedRow(s, task.id))));
  return (
    <div className="flex flex-col gap-2">
      {tasks.map((task, index) => (
        <div key={task.id} className="flex flex-col gap-1">
          {tasks.length > 1 && <WorkerName taskId={task.id} />}
          <QuietBrief>{task.spec}</QuietBrief>
          {rows[index] && <span>{rows[index]}</span>}
        </div>
      ))}
    </div>
  );
};
