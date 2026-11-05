import { shortWorkerName } from "@/app/conversation/rowWords";
import type { WorkerRow } from "@/ipc/generated";
import { formatTime } from "@/lib/format";

/**
 * A worker's state in a word or two, from its row (S14, O3): what its own CLI's events and git
 * say, the same on its Workers row, its rail row, its chip and its panel's header.
 */

export type WorkerStateKind =
  | "needsYou"
  | "limit"
  | "freshContext"
  | "quiet"
  | "working"
  | "paused"
  | "cliEnded"
  | "failed"
  | "stopped"
  | "merged"
  | "doneNotMerged"
  | "done";

export type WorkerState = { kind: WorkerStateKind; word: string };

/** How long a worker at work may say nothing before it reads quiet. */
export const QUIET_MS = 20 * 60_000;

/** Its turn has not ended yet: open, or not opened since it started. */
function atWork(row: WorkerRow): boolean {
  return row.working || (row.lastEnd === null && !row.gone);
}

/** Whether it made changes its session's branch lacks. */
function unmerged(row: WorkerRow): boolean {
  return !row.merged && (row.ahead > 0 || (!!row.diff && row.diff.insertions + row.diff.deletions + row.diff.files.length > 0));
}

/**
 * A worker's state at `now`, the first that holds of: Needs you, Limit until HH:MM, Paused (its
 * session paused it, D7), CLI ended (kept, on hand, its next message continues it), Fresh context, Quiet Nm, Working, Failed, Ended, Merged, Done · not
 * merged, Done.
 */
export function workerStateOf(row: WorkerRow, now: number): WorkerState {
  if (row.waiting.length > 0) return { kind: "needsYou", word: "Needs you" };
  if (row.limit) {
    return { kind: "limit", word: row.limit.resetsAtMs !== null ? `Limit until ${formatTime(row.limit.resetsAtMs)}` : "Limit" };
  }
  if (row.paused) return { kind: "paused", word: "Paused" };
  if (row.cliEnded) return { kind: "cliEnded", word: "Its CLI ended · next message continues it" };
  if (row.freshContext && row.working) return { kind: "freshContext", word: "Fresh context" };
  if (atWork(row)) {
    const quiet = row.lastEventAtMs === null ? 0 : now - row.lastEventAtMs;
    if (row.working && quiet >= QUIET_MS) return { kind: "quiet", word: `Quiet ${Math.floor(quiet / 60_000)}m` };
    return { kind: "working", word: "Working" };
  }
  if (row.lastEnd === "failed" || row.lastEnd === "interrupted" || (row.gone && row.lastEnd !== "done" && row.lastEnd !== "stopped")) {
    return { kind: "failed", word: "Failed" };
  }
  // Ended for good, by the user or its session: "Ended", as its thread row and card say (D199).
  if (row.lastEnd === "stopped") return { kind: "stopped", word: "Ended" };
  if (row.merged) return { kind: "merged", word: "Merged" };
  if (row.lastEnd === "done" && unmerged(row)) return { kind: "doneNotMerged", word: "Done · not merged" };
  return { kind: "done", word: "Done" };
}

/** A state word inside a line: "Needs you" reads "needs you". */
export const lowerFirst = (word: string): string => word.charAt(0).toLowerCase() + word.slice(1);

const FINISHED: ReadonlySet<WorkerStateKind> = new Set(["paused", "failed", "stopped", "merged", "doneNotMerged", "done"]);

/** Whether a worker is finished: it is the context card's, not the rail's. */
export function rowFinished(row: WorkerRow, now: number): boolean {
  return FINISHED.has(workerStateOf(row, now).kind);
}

/**
 * Whether the strip and the context card hold it among those at it: at work, waiting, or paused,
 * its toggle resuming it (D199).
 */
export function rowOnHand(row: WorkerRow, now: number): boolean {
  const { kind } = workerStateOf(row, now);
  return kind === "paused" || !FINISHED.has(kind);
}

/** Whether it waits rather than works: for the user, a limit to reset, or the message that continues it. */
export function rowWaiting(row: WorkerRow, now: number): boolean {
  const { kind } = workerStateOf(row, now);
  return kind === "needsYou" || kind === "limit" || kind === "cliEnded";
}

/**
 * How a finished worker ended in the context card: done (merged or not) or ended (stopped or
 * failed); with working and paused, the card's only words (D198, D199).
 */
export function endedWord(kind: WorkerStateKind): "Done" | "Ended" {
  return kind === "done" || kind === "doneNotMerged" || kind === "merged" ? "Done" : "Ended";
}

/** How the finished ones (not on hand) ended (`endedWord`). */
export function finishedCounts(rows: readonly WorkerRow[], now: number): { done: number; ended: number } {
  const counts = { done: 0, ended: 0 };
  for (const row of rows) {
    if (rowOnHand(row, now)) continue;
    counts[endedWord(workerStateOf(row, now).kind) === "Done" ? "done" : "ended"] += 1;
  }
  return counts;
}

/** "1 done · 1 ended": the finished workers by how they ended, zeros left out. */
export function finishedWords(rows: readonly WorkerRow[], now: number): string {
  const { done, ended } = finishedCounts(rows, now);
  return [
    [done, "done"],
    [ended, "ended"],
  ]
    .filter(([count]) => (count as number) > 0)
    .map(([count, word]) => `${count} ${word}`)
    .join(" · ");
}

/**
 * Every row's name, by id: its title made short, unique in the session, in the order the
 * workers came, as a board's workers were named (`workerName`).
 */
export function rowNames(rows: readonly WorkerRow[]): Map<string, string> {
  const names = new Map<string, string>();
  const used = new Set<string>();
  for (const row of rows.toSorted((a, b) => a.number - b.number)) {
    const base = shortWorkerName(row.title);
    let name = base;
    let ordinal = 2;
    while (used.has(name.toLowerCase())) name = `${base.split(" ").slice(0, 3).join(" ")} ${ordinal++}`;
    used.add(name.toLowerCase());
    names.set(row.id, name);
  }
  return names;
}

const named = new WeakMap<readonly WorkerRow[], Map<string, string>>();

/** A row's name among its session's rows (`rowNames`), worked out once per list. */
export function rowName(rows: readonly WorkerRow[], id: string): string | null {
  let names = named.get(rows);
  if (!names) {
    names = rowNames(rows);
    named.set(rows, names);
  }
  return names.get(id) ?? null;
}
