import { memo, useContext } from "react";

import { AgentsPanelContext, useWorkerName, WorkerGlyph } from "@/app/conversation/WorkerChip";
import { workerState } from "@/app/conversation/workerPresentation";
import type { Task, WorkerStepKind } from "@/ipc/generated";
import { useBoard } from "@/state/board";

export function lifecycleWords(task: Task) {
  const state = workerState(task);
  return { word: state === "is working" ? "started working" : state,
    tone: "quiet" as const };
}

function WorkerLink({ taskId }: { taskId: string }) {
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  return <button type="button" onClick={() => setPanel(taskId)} aria-label={`Open ${name ?? "worker"}`}
    className="hover:text-foreground focus-visible:ring-ring rounded-control inline text-start outline-none focus-visible:ring-1">
    {name ?? "Worker"}
  </button>;
}

/** Lifecycle sentences retain the event's tense; only the names open a worker. */
export const TaskRow = memo(function TaskRow({ taskId, taskIds, kind }: {
  taskId: string; taskIds?: readonly string[] | undefined; kind?: WorkerStepKind | undefined;
}) {
  const task = useBoard((s) => s.board?.tasks[taskId]);
  if (!task) return null;
  const ids = taskIds ?? [taskId];
  const word = kind === "started" || kind === "resumed" ? "started working"
    : kind === "waiting" || kind === "paused" ? "is waiting"
    : kind === "stopped" || kind === "rejected" ? "stopped"
    : kind === "failed" ? "failed" : kind ? "finished" : lifecycleWords(task).word;
  const named = ids.slice(0, ids.length > 3 ? 2 : 3);
  return <section data-slot="task-row" className="text-foreground/65 flex min-w-0 items-start gap-1.5 text-sm leading-5 select-none">
    <span aria-hidden className="inline-flex h-5 shrink-0 items-center gap-1.5">
      {ids.slice(0, 4).map((id) => <WorkerGlyph key={id} taskId={id} className="size-4" />)}
    </span>
    <span className="min-w-0">
      {named.map((id, index) => <span key={id}>
        {index > 0 && (index === named.length - 1 && ids.length <= 3 ? " and " : ", ")}
        <WorkerLink taskId={id} />
      </span>)}
      {ids.length > 3 && ` and ${ids.length - 2} more`}{" "}{word}
    </span>
  </section>;
});
