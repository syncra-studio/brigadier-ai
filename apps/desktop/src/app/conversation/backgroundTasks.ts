import type { BackgroundTask } from "@/ipc/generated";

/** Live task placement. Durable history owns all ended records (L37–L40). */
export const AGENT_STAYS_MS = 0;
export const COMMAND_STAYS_MS = 0;

/** Finished work leaves live surfaces immediately, without deleting its underlying record. */
export function taskShown(task: BackgroundTask, now: number): boolean {
  void now;
  return task.status === "running";
}

/** How a task ended, in a word, with its exit code when the CLI said one. */
export type TaskEnd = { word: "finished" | "failed" | "stopped"; exit: number | null };

/** The exit code in the CLI's words of a task's end ("… completed (exit code 0)"). */
function exitCode(summary: string | null): number | null {
  const code = summary?.match(/exit code (-?\d+)/i)?.[1];
  return code === undefined ? null : Number(code);
}

/** How `task` ended; `null` while it runs. */
export function taskEnd(task: BackgroundTask): TaskEnd | null {
  const exit = exitCode(task.summary);
  switch (task.status) {
    case "running":
      return null;
    case "completed":
      // A command the CLI calls completed with a failing code failed.
      return { word: exit !== null && exit !== 0 ? "failed" : "finished", exit };
    case "failed":
      return { word: "failed", exit };
    case "stopped":
      return { word: "stopped", exit: null };
  }
}

/** A task's end as its row says it: "finished · exit 0", "failed · exit 1", "stopped". */
export function endWords(end: TaskEnd): string {
  return end.exit === null ? end.word : `${end.word} · exit ${end.exit}`;
}
