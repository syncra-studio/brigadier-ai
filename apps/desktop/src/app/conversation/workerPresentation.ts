import type { Task } from "@/ipc/generated";

/** The worker's conversation is done when it has returned its answer, even before landing. */
export function workerDone(task: Task): boolean {
  return ["reported", "landed", "done", "rejected", "stopped", "failed"].includes(task.state);
}

export function workerWorking(task: Task): boolean {
  return ["starting", "running", "landing"].includes(task.state);
}

/**
 * Whether Stop all stops it: it runs or waits to run. The daemon's `stop_workers` stops the same
 * states; a worker landing or ready to land finishes that instead.
 */
export function workerStoppable(task: Task): boolean {
  return ["queued", "starting", "running", "blocked", "paused"].includes(task.state);
}

export function workerState(task: Task): string {
  if (task.state === "failed") return "failed";
  if (task.state === "stopped" || task.state === "rejected") return "stopped";
  if (task.state === "takenOver") return "is in your terminal";
  return workerDone(task) ? "finished" : workerWorking(task) ? "is working" : "is waiting";
}

/** Waiting stays still and tells the user what would let the worker continue. */
export function workerPreview(task: Task): string | null {
  if (workerDone(task)) return task.state === "failed" || task.state === "stopped" || task.state === "rejected"
    ? workerState(task) : null;
  if (workerWorking(task)) {
    const objective = task.spec.split("\n").map((line) => line.trim()).find((line) => /^(?:goal|objective):/i.test(line));
    return objective ? objective.replace(/^(?:goal|objective):\s*/i, "").replace(/task-\d+/gi, "worker").slice(0, 60) : "Working";
  }
  if (task.state === "takenOver") return "Working in your terminal";
  if (task.quotaWait) return "Waiting for model quota";
  const reason = task.blockedReason ?? "";
  if (/code review/i.test(reason)) return "Waiting for its code review";
  if (/outline|plan|review/i.test(reason)) return "Waiting for its plan to be reviewed";
  if (/approval|permission|readyToLand/i.test(reason) || task.state === "readyToLand") return "Waiting for your approval";
  if (task.state === "queued" || /slot|capacity|worker|step/i.test(reason)) return "Waiting for a free slot";
  if (task.state === "paused") return "Waiting to continue";
  return "Waiting for an answer";
}
