import type { BackgroundTask } from "@/ipc/generated";

/**
 * The CLI's own background tasks as the context card's Background rows show them: only running
 * tasks have rows. Completed, failed and stopped tasks leave immediately; the thread keeps
 * their history.
 */

/** Whether `task` is still running and can have a Background row. */
export function taskShown(task: BackgroundTask): boolean {
  return task.status === "running";
}
