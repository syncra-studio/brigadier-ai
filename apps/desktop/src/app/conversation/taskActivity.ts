import type { DiffStat, Task } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";

const TERMINAL = new Set<Task["state"]>(["landed", "done", "rejected", "stopped", "failed"]);

const SETTLED_WORKER = new Set<Task["state"]>(["reported", "readyToLand"]);

function hasTaskActivity(task: Task): boolean {
  return !TERMINAL.has(task.state);
}

function userPaused(task: Task): boolean {
  return task.state === "paused" && !task.quotaWait && !task.blockedReason?.trim();
}

/** Settled workers keep their activity visible without subscribing to the live clock. */
export function taskActivityTicks(task: Task): boolean {
  return hasTaskActivity(task) && !userPaused(task) && (!!task.quotaWait || !SETTLED_WORKER.has(task.state));
}

/** A wait always takes precedence over the worker's last provider activity. */
export function taskWaitWords(task: Task): string | null {
  if (!hasTaskActivity(task)) return null;
  if (task.quotaWait) {
    const reason = task.quotaWait.reason;
    const provider = /claude/i.test(reason) ? "Claude" : /codex/i.test(reason) ? "Codex" : null;
    return `Waiting for ${provider ? `${provider} ` : ""}quota: ${reason}`;
  }
  if (task.state === "takenOver") return "Working in your terminal";
  if (userPaused(task)) return "Paused";
  if (task.state === "queued") {
    return task.blockedReason ? `Queued: ${task.blockedReason}` : "Queued: waiting to start";
  }
  if (task.state === "blocked" || task.state === "paused") {
    const reason = task.blockedReason?.trim();
    return reason ? (/^waiting\b/i.test(reason) ? reason : `Waiting: ${reason}`) : "Waiting";
  }
  if (task.state === "readyToLand") return task.run ? "Held" : "Ready to land";
  return null;
}

export type ActivitySource = {
  task: Task | undefined;
  activity?: string | undefined;
  summary?: string | undefined;
  diff?: DiffStat | undefined;
};

export type ActivityLines = {
  first: string;
  second: string;
  firstWorking: boolean;
  secondWorking: boolean;
};

function semantic(source: ActivitySource): string {
  const task = source.task!;
  const wait = taskWaitWords(task);
  if (wait) return wait;
  const activity = source.activity || source.summary;
  if (activity && (task.state === "running" || task.state === "starting")) return activity;
  switch (task.state) {
    case "starting":
      return "Starting…";
    case "running":
      return "Thinking…";
    case "landing":
      return "Landing";
    case "reported":
      return "Finished, handing back";
    default:
      return "Waiting";
  }
}

export function activelyWorking(source: ActivitySource): boolean {
  return (
    !!source.task && !taskWaitWords(source.task) &&
    (source.task.state === "running" || source.task.state === "starting") &&
    !/^(Waiting|Retrying|Error)\b/i.test(source.activity ?? "")
  );
}

function timed(source: ActivitySource, text: string, now: number): string {
  const task = source.task;
  if (!task) return text;
  const attempt = task.attempts.at(-1);
  const start = task.quotaWait?.sinceMs ?? attempt?.startedAtMs ?? task.createdAtMs;
  const end = userPaused(task)
    ? task.updatedAtMs
    : taskActivityTicks(task) ? now : attempt?.endedAtMs ?? task.updatedAtMs;
  const elapsed = formatDuration(Math.max(0, end - start));
  const diff = source.diff ?? task.candidate?.diffStat;
  const changes = diff && diff.insertions + diff.deletions > 0 ? ` · +${diff.insertions} −${diff.deletions}` : "";
  return `${text} · ${elapsed}${changes}`;
}

/** At most two lines of what a worker is at. */
export function taskActivityLines(source: ActivitySource, now: number): ActivityLines {
  const empty = { first: "", second: "", firstWorking: false, secondWorking: false };
  const task = source.task;
  if (!task || !hasTaskActivity(task)) return empty;
  return { ...empty, first: timed(source, semantic(source), now), firstWorking: activelyWorking(source) };
}
