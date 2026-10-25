import { changesDetail, detailOf, isPlumbing, stepWords } from "@/app/conversation/activity/words";
import { lowerFirst, type WorkerState, workerStateOf } from "@/app/conversation/activity/workerState";
import type { ProviderEvent, StepAtWork, WorkerDoing, WorkerRow } from "@/ipc/generated";

/**
 * A worker's live line, the same on its Workers strip row and its context card row: while it
 * works, what it does right now in the thread's words ("Running pnpm build", "Editing foo.ts"),
 * its newest call at work; between its calls, what it last did (its last call, "Thinking…",
 * "Writing…", the first line of what it wrote), as the rows said before the strip-down (D39);
 * else, and whenever it isn't working, its state word.
 */
export function workerNow(row: WorkerRow, now: number): WorkerNow {
  const state = workerStateOf(row, now);
  if (state.kind !== "working") return state;
  const step = (row.steps ?? []).findLast((each) => !isPlumbing(each.name));
  if (step) return { kind: "working", word: stepWords({ ...step, status: "inProgress" }).doing, step: true };
  const doing = row.doing;
  switch (doing?.type) {
    case "call":
      return isPlumbing(doing.step.name) ? state : { kind: "working", word: stepWords({ ...doing.step, status: "inProgress" }).doing, step: true };
    case "thinking":
      return { kind: "working", word: "Thinking…" };
    case "writing":
      return { kind: "working", word: "Writing…" };
    case "said":
      return { kind: "working", word: doing.line, step: true };
    default:
      return state;
  }
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

/**
 * What a thread's item says its agent last did, as the engine says a worker's
 * (`worker_rows::doing_of`): a call, ended or not, thinking, writing, or the first line of what
 * it wrote; `null` for anything else.
 */
export function doingOf(event: ProviderEvent): WorkerDoing | null {
  switch (event.type) {
    case "command":
      return event.command ? { type: "call", step: { name: "shell", detail: event.command } } : null;
    case "toolCall":
      return { type: "call", step: { name: event.name, detail: detailOf(event.input) } };
    case "fileChanges":
      return { type: "call", step: { name: "apply_patch", detail: changesDetail(event.changes.map((change) => change.path)) } };
    case "image":
      return { type: "call", step: { name: "imagegen", detail: event.prompt } };
    case "reasoning":
    case "reasoningDelta":
      return { type: "thinking" };
    case "messageDelta":
      return { type: "writing" };
    case "message": {
      const line = event.role === "assistant" ? event.text.split("\n").map((each) => each.trim()).find(Boolean) : undefined;
      return line ? { type: "said", line: line.slice(0, 240) } : null;
    }
    default:
      return null;
  }
}
