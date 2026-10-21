import { useContext } from "react";
import { useShallow } from "zustand/react/shallow";

import { taskState } from "@/app/conversation/rowWords";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { useBoard } from "@/state/board";

const NUL = "\u0000";

/** A phase's workers inside the run card, one line each, each opening its worker. */
export function PhaseWork({ taskIds }: { taskIds: readonly string[] }) {
  const { setPanel } = useContext(AgentsPanelContext);
  // Each line as "task id NUL state NUL title", so the card re-renders only when one changes.
  const lines = useBoard(
    useShallow((s) =>
      taskIds.flatMap((id) => {
        const task = s.board?.tasks[id];
        // A check opens from what it checked; a phase's own checks list here.
        if (!task || (task.gateLink !== null && task.gateLink.owner.type !== "phase")) return [];
        return [[task.id, taskState(task).word, task.title].join(NUL)];
      }),
    ),
  );
  if (lines.length === 0) return null;
  return (
    <ul className="flex flex-col whitespace-normal">
      {lines.map((line) => {
        const [taskId = "", word = "", title = ""] = line.split(NUL);
        return (
          <li key={taskId} className="flex min-w-0">
            <button
              type="button"
              title={title}
              onClick={() => setPanel(taskId)}
              className="hover:text-foreground rounded-control flex min-w-0 items-center gap-1 text-start"
            >
              <span className="text-foreground/80 min-w-0 truncate">{title}</span>
              <span className="shrink-0">· {word}</span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
