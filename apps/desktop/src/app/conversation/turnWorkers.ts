import type { ThreadTurn, WorkerRow } from "@/ipc/generated";

/**
 * The workers a turn started that still work: its work group stays open and live until they
 * finish, then folds as usual (L3). A worker belongs to the turn it started in, by its start
 * against the turns' starts: the turns the CLI begins on its own (a background `brigadier wait`
 * ending) are in the same turn, so the workers they start are too.
 *
 * A worker still works while its CLI is on the run it was started for, or waits on the user. In
 * the newest turn, a worker the turn sent back to work counts again; an older turn's worker
 * that a later turn sent more work never opens the older turn again.
 */
export function workingWorkers(
  workers: readonly WorkerRow[],
  turns: readonly Pick<ThreadTurn, "startedAtMs">[],
  startedAtMs: number,
): number {
  const next = turns.find((turn) => turn.startedAtMs !== null && turn.startedAtMs > startedAtMs)?.startedAtMs ?? null;
  const newest = next === null;
  return workers.filter((worker) => {
    if (worker.startedAtMs < startedAtMs || (next !== null && worker.startedAtMs >= next) || worker.gone) return false;
    if (worker.lastEnd === null) return true;
    return newest && (worker.working || worker.waiting.length > 0);
  }).length;
}
