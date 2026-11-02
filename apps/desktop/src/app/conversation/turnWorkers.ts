import type { ThreadTurn, WorkerRow } from "@/ipc/generated";

/**
 * The workers a turn started that still work: its work group stays open and live until they
 * finish, then folds as usual (L3). A worker belongs to the turn it started in, by its start
 * against the turns' starts: the turns the CLI begins on its own (a background `brigadier wait`
 * ending) are in the same turn, so the workers they start are too. `starts`: when each of the
 * block's turns started (Brigadier's word of its workers joins the block as a turn more).
 *
 * A worker still works while its CLI is on the run it was started for, or waits on the user. In
 * the newest turn, a worker the turn sent back to work counts again, wherever it started (a
 * resumed or switched one, D151); an older turn's worker that a later turn sent more work never
 * opens the older turn again. A paused one holds nothing (D7, D118).
 */
/**
 * When the turn after a block's, its turns started at `starts`, began; `null` for the newest
 * block. Its first turn is its own though it began after the block did: the block starts with
 * its message, sent before its CLI's turn began (`startBy`).
 */
export function nextTurnStart(turns: readonly Pick<ThreadTurn, "startedAtMs">[], starts: readonly number[]): number | null {
  const startedAtMs = Math.min(...starts);
  const first = turns.find((turn) => turn.startedAtMs !== null && turn.startedAtMs >= startedAtMs);
  return turns.find((turn) => turn !== first && turn.startedAtMs !== null && turn.startedAtMs > startedAtMs && !starts.includes(turn.startedAtMs))?.startedAtMs ?? null;
}

export function workingWorkers(
  workers: readonly WorkerRow[],
  turns: readonly Pick<ThreadTurn, "startedAtMs">[],
  starts: readonly number[],
  endedAtMs: number | null = null,
  owed = false,
): number {
  return workingWorkerRows(workers, turns, starts, endedAtMs, owed).length;
}

/**
 * The rows of [`workingWorkers`]. `owed`: the CLI's turn that carries a word of its workers
 * hasn't started yet (`CliJobs::waking`): one that ended after the block's turns did still holds
 * it until then, so it never reads over in between (D24, D94).
 */
export function workingWorkerRows(
  workers: readonly WorkerRow[],
  turns: readonly Pick<ThreadTurn, "startedAtMs">[],
  starts: readonly number[],
  endedAtMs: number | null = null,
  owed = false,
): WorkerRow[] {
  if (starts.length === 0) return [];
  const startedAtMs = Math.min(...starts);
  const next = nextTurnStart(turns, starts);
  const newest = next === null;
  return workers.filter((worker) => {
    // None started after its turns ended is its: the next turn may not be in `turns` yet. A paused
    // one runs nothing (D7, D118), as the daemon's queue reads it.
    if ((next !== null && worker.startedAtMs >= next) || (endedAtMs !== null && worker.startedAtMs > endedAtMs) || worker.gone || worker.paused) return false;
    // One an earlier block started (resumed or switched since) holds the newest block while it
    // works, as any working worker holds the daemon's queue (D24, D151).
    if (worker.lastEnd === null && worker.startedAtMs >= startedAtMs) return true;
    const unheard = owed && endedAtMs !== null && (worker.lastEventAtMs ?? 0) > endedAtMs;
    return newest && (worker.working || worker.waiting.length > 0 || unheard);
  });
}

/**
 * Whether the turn that ran from `startedAtMs` to `endedAtMs` is open again (the end of a
 * background job it started woke it), its thread's open turn starting at `openStart`. The thread
 * says so a draw before its block does, so its work never folds between the two (D24).
 */
export function reopened(openStart: number | null, startedAtMs: number, endedAtMs: number | null): boolean {
  return openStart !== null && endedAtMs !== null && openStart >= startedAtMs && openStart < endedAtMs;
}
