import { runsBrigadier, taskCommand } from "@/app/conversation/backgroundRows";
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
 * end starts begins (`CliJobs::waking`).
 */
export function turnCommands(
  view: Pick<ThreadView, "background" | "entries" | "jobs">,
  turns: readonly Pick<ThreadTurn, "startedAtMs">[],
  starts: readonly number[],
): number {
  const jobs = view.jobs;
  if (starts.length === 0 || !jobs || (!jobs.holding?.length && !jobs.waking)) return 0;
  const startedAtMs = Math.min(...starts);
  const next = turns.find((turn) => turn.startedAtMs !== null && turn.startedAtMs > startedAtMs && !starts.includes(turn.startedAtMs))?.startedAtMs ?? null;
  const entries = view.entries;
  const background = view.background ?? [];
  const ours = (task: BackgroundTask) => {
    const call = task.itemId === null ? undefined : entries.find(({ event, parent }) => !parent && "itemId" in event && event.itemId === task.itemId);
    const at = call?.atMs ?? task.startedAtMs;
    return at >= startedAtMs && (next === null || at < next);
  };
  const holding = background.filter((task) => jobs.holding?.includes(task.id) && ours(task)).length;
  if (holding > 0 || !jobs.waking || next !== null) return holding;
  // Its end owes the turn it starts: the newest block that started it waits for that turn.
  const owed = background.some((task) => task.status !== "running" && task.kind !== "agent" && ours(task) && !excluded(task, entries));
  return owed ? 1 : 0;
}

/** A wait or a preview call: covered by the workers' own hold, or a row of its own. */
function excluded(task: BackgroundTask, entries: readonly FileEntry[]): boolean {
  const command = taskCommand(task, entries) ?? "";
  return runsBrigadier(command, "wait") || runsBrigadier(command, "preview");
}
