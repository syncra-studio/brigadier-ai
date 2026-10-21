import type { FC } from "react";

import { QuietBrief } from "@/app/conversation/activity/QuietBrief";
import { WorkerName } from "@/app/conversation/activity/WorkerName";
import type { Task } from "@/ipc/generated";

/** A start's detail: each worker's brief, in a quiet box. */
export const Briefs: FC<{ tasks: readonly Task[] }> = ({ tasks }) => (
  <div className="flex flex-col gap-2">
    {tasks.map((task) => (
      <div key={task.id} className="flex flex-col gap-1">
        {tasks.length > 1 && <WorkerName taskId={task.id} />}
        <QuietBrief>{task.spec}</QuietBrief>
      </div>
    ))}
  </div>
);
