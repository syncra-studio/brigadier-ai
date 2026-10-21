import { type FC, useContext, useEffect, useRef } from "react";
import { useShallow } from "zustand/react/shallow";

import { WORKERS_LABEL } from "@/app/side-panel/Agents";
import { Counts } from "@/components/shared";
import { outcomeWords, workerDone, workerWorking } from "@/app/conversation/workerPresentation";
import { AgentsPanelContext, useWorkerName } from "@/app/conversation/WorkerChip";
import { SummaryRowButton, SummarySection, WorkerGlyph } from "@/components/conversation";
import type { DiffStat, Task } from "@/ipc/generated";
import { refreshWorkerDiffs } from "@/state/actions";
import { useBoard, workerNote } from "@/state/board";

/**
 * The session's worker summary and background diff refresh.
 */

/**
 * States in which a write task's +N −N is read from its worktree: it still changes, or it
 * reported and its candidate commit isn't made yet.
 */
const EDITING: ReadonlySet<Task["state"]> = new Set(["running", "blocked", "paused", "reported", "takenOver"]);

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

/** How many workers at work still get a line each; more read as one row. */
const WORKER_LINES = 3;

/**
 * A worker at work: its glyph, its name, a soft "working" while it works, and its +N −N; under
 * them its progress note when it posted one.
 */
const WorkerLineRow: FC<{ task: Task }> = ({ task }) => {
  const name = useWorkerName(task.id);
  const stat = useBoard((s) => workerStat(task, s.board?.diffs));
  const note = useBoard((s) => workerNote(s.board, task.id));
  const { setPanel } = useContext(AgentsPanelContext);
  return (
    <SummaryRowButton
      data-slot="workers-summary-worker"
      title={name ?? undefined}
      onClick={() => setPanel(task.id)}
      // With a note, the glyph stays level with the name.
      className={note ? "h-auto items-start py-1.25" : undefined}
      icon={<span className="flex h-lh items-center"><WorkerGlyph taskId={task.id} className="size-4" /></span>}
      meta={stat && stat.insertions + stat.deletions > 0 && <Counts insertions={stat.insertions} deletions={stat.deletions} />}
    >
      <span className="block truncate">
        {name}{workerWorking(task) && <span className="text-muted-foreground shimmer"> working</span>}
      </span>
      {note && <span data-slot="workers-summary-note" className="text-muted-foreground block truncate text-xs">{note}</span>}
    </SummaryRowButton>
  );
};

/**
 * The session's workers in the context card: one row of avatars with "N working" or "N done"
 * (the 10-07 row), or, while a few work, a line each. A click opens the Workers panel, or the
 * worker's own; what a worker runs shows there, not here.
 */
export function WorkersSummary({ conversationId }: { conversationId: string }) {
  const tasks = useBoard(useShallow((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.tasks).toSorted((a, b) => b.createdAtMs - a.createdAtMs)
    : []));
  const { panel, setPanel } = useContext(AgentsPanelContext);
  if (!tasks.length) return null;
  const active = tasks.filter((task) => !workerDone(task));
  const finished = outcomeWords(tasks);
  const avatars = active.length ? active : tasks;
  const lines = active.length > 0 && active.length <= WORKER_LINES;
  return (
    <SummarySection foldKey="workers" title={WORKERS_LABEL} aria-label="Session workers">
      {lines ? (
        <>
          {active.map((task) => <WorkerLineRow key={task.id} task={task} />)}
          {finished && (
            <SummaryRowButton
              aria-label="Open workers"
              data-slot="workers-summary"
              data-state={panel !== undefined ? "open" : "closed"}
              onClick={() => setPanel(null)}
              muted
              icon={<WorkerGlyph taskId={tasks.find(workerDone)!.id} className="size-4" />}
            >
              {finished}
            </SummaryRowButton>
          )}
        </>
      ) : (
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
          meta={active.length > 0 && finished ? finished : undefined}
        >
          {active.length ? `${active.length} working` : finished}
        </SummaryRowButton>
      )}
    </SummarySection>
  );
}
