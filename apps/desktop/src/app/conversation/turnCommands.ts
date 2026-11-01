import { runsBrigadier, taskCommand } from "@/app/conversation/backgroundRows";
import { nextTurnStart } from "@/app/conversation/turnWorkers";
import type { BackgroundTask, FileEntry, ThreadTurn, ThreadView } from "@/ipc/generated";

/**
 * The background commands a session's block started that still hold it (D21), as its workers
 * do (`workingWorkers`): its work group stays open and live, "Waiting for a command", until they
 * end and the turn their end starts is over; then it folds as usual.
 *
 * Which commands hold is the engine's word (`CliJobs::holding`: not a subagent, a
 * `brigadier wait` or a `brigadier preview` call). A command belongs to the block whose turns
 * its call came in: found by the task's item id among the thread's items, else by when it
 * started (a Codex terminal is seen only when Codex lists it, maybe after a later turn began).
 * One that ended while no turn was open holds the newest block it started in until the turn its
 * end starts begins (`CliJobs::waking`). None started after the block's turns ended (`endedAtMs`)
 * is its: the next turn may not be in `turns` yet while its items already are.
 */
export function turnCommands(
  view: Pick<ThreadView, "background" | "entries" | "jobs">,
  turns: readonly Pick<ThreadTurn, "startedAtMs">[],
  starts: readonly number[],
  endedAtMs: number | null = null,
): number {
  return turnCommandTasks(view, turns, starts, endedAtMs).length;
}

/** The commands of [`turnCommands`]: an ended one whose end's turn is owed counts once. */
export function turnCommandTasks(
  view: Pick<ThreadView, "background" | "entries" | "jobs">,
  turns: readonly Pick<ThreadTurn, "startedAtMs">[],
  starts: readonly number[],
  endedAtMs: number | null = null,
): BackgroundTask[] {
  const jobs = view.jobs;
  if (starts.length === 0 || !jobs || (!jobs.holding?.length && !jobs.waking)) return [];
  const startedAtMs = Math.min(...starts);
  const next = nextTurnStart(turns, starts);
  const entries = view.entries;
  const background = view.background ?? [];
  const ours = (task: BackgroundTask) => {
    const call = task.itemId === null ? undefined : entries.find(({ event, parent }) => !parent && "itemId" in event && event.itemId === task.itemId);
    const at = call?.atMs ?? task.startedAtMs;
    return at >= startedAtMs && (next === null || at < next) && (endedAtMs === null || at <= endedAtMs);
  };
  const holding = background.filter((task) => jobs.holding?.includes(task.id) && ours(task));
  if (holding.length > 0 || !jobs.waking || next !== null) return holding;
  // Its end owes the turn it starts: the newest block that started it waits for that turn.
  const owed = background.findLast((task) => task.status !== "running" && task.kind !== "agent" && ours(task) && !excluded(task, entries));
  return owed ? [owed] : [];
}

/** A wait or a preview call: covered by the workers' own hold, or a row of its own. */
function excluded(task: BackgroundTask, entries: readonly FileEntry[]): boolean {
  const command = taskCommand(task, entries) ?? "";
  return runsBrigadier(command, "wait") || runsBrigadier(command, "preview");
}
