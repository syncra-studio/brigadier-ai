import { changesDetail, detailOf, isPlumbing, stepWords } from "@/app/conversation/activity/words";
import { lowerFirst, type WorkerState, workerStateOf } from "@/app/conversation/activity/workerState";
import type { ProviderEvent, StepAtWork, WorkerRow } from "@/ipc/generated";

/**
 * A worker's live line, the same on its Workers strip row and its context card row: while it
 * works, what it does right now in the thread's words ("Running pnpm build", "Editing foo.ts"),
 * its newest call at work; between its calls, and whenever it isn't working, its state word.
 */
export function workerNow(row: WorkerRow, now: number): WorkerNow {
  const state = workerStateOf(row, now);
  if (state.kind !== "working") return state;
  const step = (row.steps ?? []).findLast((each) => !isPlumbing(each.name));
  return step ? { kind: "working", word: stepWords({ ...step, status: "inProgress" }).doing, step: true } : state;
}

/** A worker's state, or what it does: `step` when its words are its call's. */
export type WorkerNow = WorkerState & { step?: true };

/**
 * The line's words after its name ("Pixel needs you", "Pixel running pnpm build"): lowercase
 * but for a name they start with ("Pencil: batch design").
 */
export function nowInLine(state: WorkerNow): string {
  return state.step && !/^\p{Lu}\p{Ll}+ing\b/u.test(state.word) ? state.word : lowerFirst(state.word);
}

/**
 * A call at work, as the engine names a worker's (`worker_rows::step_of`), from an item of a
 * CLI's thread; `null` for what is no call, or one that ended.
 */
export function stepAtWork(event: ProviderEvent): StepAtWork | null {
  switch (event.type) {
    case "command":
      return event.status === "inProgress" && event.command ? { name: "shell", detail: event.command } : null;
    case "toolCall":
      return event.status === "inProgress" ? { name: event.name, detail: detailOf(event.input) } : null;
    case "fileChanges":
      return event.status === "inProgress" ? { name: "apply_patch", detail: changesDetail(event.changes.map((change) => change.path)) } : null;
    case "image":
      return event.status === "inProgress" ? { name: "imagegen", detail: event.prompt } : null;
    default:
      return null;
  }
}
