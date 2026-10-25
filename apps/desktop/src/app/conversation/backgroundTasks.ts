import type { BackgroundTask } from "@/ipc/generated";

/**
 * The CLI's own background tasks as the context card's Background rows show them, by the CLI's
 * own rules (Claude Code's `/tasks`; docs/STRIP-DOWN-PLAN.md: "behave exactly like Claude Code's
 * own"): a running task has its row; a command that ended keeps it about 5 min, saying how it
 * ended ("finished · exit 0", "failed", "stopped"), a subagent 30 s, then it goes (D21). A Codex
 * background terminal reads the same, ended when Codex stops listing it.
 */

/** How long a finished subagent keeps its row. */
export const AGENT_STAYS_MS = 30_000;
/** How long a command that ended keeps its row. */
export const COMMAND_STAYS_MS = 5 * 60_000;

/** Whether `task` still has its row at `now`. */
export function taskShown(task: BackgroundTask, now: number): boolean {
  if (task.status === "running") return true;
  const ended = task.endedAtMs ?? now;
  return now - ended < (task.kind === "agent" ? AGENT_STAYS_MS : COMMAND_STAYS_MS);
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
