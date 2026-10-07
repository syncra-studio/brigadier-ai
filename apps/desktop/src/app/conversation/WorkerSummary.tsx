import { type FC, useContext, useEffect, useRef } from "react";
import { useShallow } from "zustand/react/shallow";

import { WORKERS_LABEL } from "@/app/conversation/Agents";
import { workerDone } from "@/app/conversation/workerPresentation";
import {
  AgentsPanelContext,
  WorkerGlyph,
} from "@/app/conversation/WorkerChip";
import { SummaryRowButton, SummarySection } from "@/components/assistant-ui/elements/summary-section";
import type { DiffStat, Task } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { refreshWorkerDiffs } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The session's worker summary and background diff refresh.
 */

/** "+12 −3", once there is a change; `monochrome` for a total, in the text's colour. */
export const Changes: FC<{ insertions: number; deletions: number; monochrome?: boolean }> = ({
  insertions,
  deletions,
  monochrome = false,
}) =>
  insertions + deletions > 0 ? (
    <span className="shrink-0 font-mono text-xs tabular-nums">
      <span className={cn(!monochrome && "text-success")}>+{insertions}</span>{" "}
      <span className={cn(!monochrome && "text-destructive")}>−{deletions}</span>
    </span>
  ) : null;

/**
 * States in which a write task's +N −N is read from its worktree: it still changes, or it
 * reported and its candidate commit isn't made yet.
 */
const EDITING: ReadonlySet<Task["state"]> = new Set(["running", "blocked", "paused", "reported"]);

function writes(task: Task): boolean {
  return task.kind === "implement" || task.kind === "merge";
}

/** A worker's +N −N: its worktree so far while it works, then its candidate commit's. */
export function workerStat(
  task: Task,
  diffs: Readonly<Record<string, DiffStat>> | undefined,
): DiffStat | undefined {
  return (EDITING.has(task.state) ? diffs?.[task.id] : undefined) ?? task.candidate?.diffStat;
}

/** The soonest a burst of edits reads the worktrees again, and the least time between reads. */
const DIFF_SETTLE_MS = 500;
const DIFF_INTERVAL_MS = 3_000;

/**
 * Keeps the board's live +N −N of the conversation's workers current: read again after
 * a worker at work on a change edits files, runs a command or changes state, at most every
 * few seconds.
 * It renders nothing, so what it watches re-renders only itself.
 */
export function WorkerDiffs({ conversationId }: { conversationId: string }) {
  // Changes whenever a write task at work edits files, runs a command or changes state.
  const key = useBoard((s) => {
    const board = s.board;
    if (board?.conversationId !== conversationId) return "";
    let edited = "";
    for (const task of Object.values(board.tasks)) {
      if (writes(task) && EDITING.has(task.state)) {
        edited += `${task.id}:${task.state}:${board.edits[task.id] ?? 0};`;
      }
    }
    return edited;
  });
  const last = useRef(0);
  useEffect(() => {
    if (!key) return;
    const delay = Math.max(DIFF_SETTLE_MS, last.current + DIFF_INTERVAL_MS - Date.now());
    const timer = setTimeout(() => {
      last.current = Date.now();
      // Until git can tell (a worktree being set up), the last read stays.
      refreshWorkerDiffs(conversationId).catch(() => {});
    }, delay);
    return () => clearTimeout(timer);
  }, [conversationId, key]);
  return null;
}

/** The summary keeps every worker openable, including helpers and finished workers. */
export function WorkersSummary({ conversationId }: { conversationId: string }) {
  const tasks = useBoard(useShallow((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.tasks).toSorted((a, b) => b.createdAtMs - a.createdAtMs)
    : []));
  const { panel, setPanel } = useContext(AgentsPanelContext);
  if (!tasks.length) return null;
  const active = tasks.filter((task) => !workerDone(task));
  const done = tasks.length - active.length;
  const avatars = active.length ? active : tasks;
  return (
    <SummarySection foldKey="workers" title={WORKERS_LABEL} aria-label="Session workers">
      <SummaryRowButton
        aria-label="Open workers"
        data-slot="workers-summary"
        // Washed while the Workers tab shows, as on hover.
        data-state={panel !== undefined ? "open" : "closed"}
        onClick={() => setPanel(null)}
        className="gap-1.5"
        icon={
          <span className="flex items-center gap-1.5">
            {avatars.slice(0, 4).map((task) => <WorkerGlyph key={task.id} taskId={task.id} className="size-4" />)}
          </span>
        }
        meta={active.length > 0 && done > 0 ? `${done} done` : undefined}
      >
        {active.length ? `${active.length} working` : `${done} done`}
      </SummaryRowButton>
    </SummarySection>
  );
}
