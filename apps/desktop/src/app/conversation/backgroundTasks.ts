import type { BackgroundTask } from "@/ipc/generated";

/**
 * The CLI's own background tasks as the context card's Background rows show them, by the CLI's
 * own rules (Claude Code's `/tasks`, Codex's `/ps`): a running task has its row; a subagent or
 * any other task that finished stays 30 s, a command about 5 min after it exits; a failed or
 * stopped task leaves at once, and a Codex background terminal leaves when Codex stops listing it.
 */

/** How long a finished subagent keeps its row. */
export const AGENT_STAYS_MS = 30_000;
/** How long a command that exited keeps its row. */
export const SHELL_STAYS_MS = 5 * 60_000;

/** Whether `task` still has its row at `now`. */
export function taskShown(task: BackgroundTask, now: number): boolean {
  switch (task.status) {
    case "running":
      return true;
    case "failed":
    case "stopped":
      return false;
    case "completed": {
      if (task.kind === "terminal") return false;
      const ended = task.endedAtMs ?? now;
      return now - ended < (task.kind === "shell" ? SHELL_STAYS_MS : AGENT_STAYS_MS);
    }
  }
}
