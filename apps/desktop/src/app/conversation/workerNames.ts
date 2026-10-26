import { useCallback, useMemo } from "react";

import { rowName } from "@/app/conversation/activity/workerState";
import { namedMentions, workerName } from "@/app/conversation/rowWords";
import { workersOf } from "@/state/nativeWorkers";
import { useBoard } from "@/state/board";
import { useCliThread, workerRowIn } from "@/state/cliThread";

/**
 * What the user reads a worker as, rather than its `task-N`: its name, alone or in their own
 * words. Kept apart from the chip (WorkerChip.tsx) so the conversation components can name
 * workers too.
 */

/**
 * What a worker is called, rather than its `task-N`: its row's name among its session's rows
 * (`rowName`). A worker no thread read lists yet (a view still loading, a stored run's) keeps the
 * board's name for it (`workerName`).
 */
export function useWorkerName(taskId: string): string | null {
  const fromRow = useCliThread((s) => rowNameIn(s.views, taskId));
  const fromBoard = useBoard((s) => {
    if (fromRow !== null) return null;
    const task = s.board?.tasks[taskId];
    return task && s.board ? workerName(s.board.tasks, task) : null;
  });
  return fromRow ?? fromBoard;
}

/** A worker's name among its session's rows, if a thread read lists it (see `useWorkerName`). */
function rowNameIn(views: Parameters<typeof workerRowIn>[0], taskId: string): string | null {
  const found = workerRowIn(views, taskId);
  return found ? rowName(workersOf(found.conversationId, views[found.conversationId]!), taskId) : null;
}

/**
 * The open session's workers' names (`useWorkerName`) by number, the same map until one
 * changes: worked out as `number\tname` lines, which compare as text.
 */
function useNamesByNumber(): ReadonlyMap<number, string> {
  // Not the whole board, which changes with every streamed word.
  const conversationId = useBoard((s) => s.board?.conversationId);
  const tasks = useBoard((s) => s.board?.tasks);
  const lines = useCliThread((s) => {
    if (!conversationId || !tasks) return "";
    const view = s.views[conversationId];
    const rows = view ? workersOf(conversationId, view) : null;
    return Object.values(tasks)
      .map((task) => `${task.number}\t${(rows && rowName(rows, task.id)) ?? workerName(tasks, task)}`)
      .join("\n");
  });
  return useMemo(() => new Map(lines ? lines.split("\n").map((line) => {
    const tab = line.indexOf("\t");
    return [Number(line.slice(0, tab)), line.slice(tab + 1)] as const;
  }) : []), [lines]);
}

/**
 * The user's own words as they read them, where they show as text rather than in a bubble (a
 * queued message, a notice, a preview): each `@task-N` by its worker's name (`namedMentions`).
 */
export function useNamedMentions(): (text: string) => string {
  const names = useNamesByNumber();
  return useCallback((text: string) => namedMentions(text, names), [names]);
}
