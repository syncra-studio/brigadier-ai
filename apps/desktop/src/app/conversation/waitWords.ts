import { brigadierCalls, toolName } from "@/app/conversation/activity/words";
import { rowNames } from "@/app/conversation/activity/workerState";
import { taskCommand } from "@/app/conversation/backgroundRows";
import { commandWords, type StatusHead } from "@/app/conversation/liveStatus";
import { workersWaitWords } from "@/app/conversation/requestTime";
import { turnCommandTasks } from "@/app/conversation/turnCommands";
import { workingWorkerRows } from "@/app/conversation/turnWorkers";
import type { BackgroundTask, FileEntry, ThreadView, WorkerRow } from "@/ipc/generated";
import { backgroundOf, runningSubagentTasks } from "@/state/nativeWorkers";

/** A wait on one background task, named as its Background row names it (D106). */
function taskWait(task: BackgroundTask, entries: readonly FileEntry[]): StatusHead {
  if (task.kind === "agent") return { text: `Waiting for subagent: ${task.label || "a subagent"}`, tone: "still" };
  return { text: `Waiting for ${commandWords(taskCommand(task, entries) ?? task.label)}`, tone: "still" };
}

function subagentsWait(labels: readonly string[]): StatusHead {
  const [only] = labels;
  const text = labels.length > 1 ? `Waiting for ${labels.length} subagents` : only ? `Waiting for subagent: ${only}` : "Waiting for a subagent";
  return { text, tone: "still" };
}

/** A wait on workers, named by the Workers card's names when it is one. */
function workersWait(rows: readonly WorkerRow[], all: readonly WorkerRow[]): StatusHead {
  const name = rows.length === 1 ? rowNames(all).get(rows[0]!.id) : undefined;
  return { text: name ? `Waiting for ${name}` : workersWaitWords(Math.max(rows.length, 1)), tone: "busy" };
}

function inputOf(entry: FileEntry): Record<string, unknown> {
  if (entry.event.type !== "toolCall") return {};
  try {
    const input: unknown = JSON.parse(entry.event.input ?? "null");
    return input && typeof input === "object" ? (input as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** The command a call runs: a command item's, or a shell tool call's `command`. */
function callCommand(entry: FileEntry): string | null {
  if (entry.event.type === "command") return entry.event.command;
  const command = inputOf(entry).command ?? inputOf(entry).cmd;
  return typeof command === "string" ? command : null;
}

/** The workers a `brigadier wait` call waits on: the ids it names (as `brigadier` reads a short id), else every one still working. */
function waitedWorkers(command: string, workers: readonly WorkerRow[]): WorkerRow[] {
  const after = command.split(/\bbrigadier\s+wait\b/)[1] ?? "";
  const ids = after.split(/[;&|\n]/)[0]!.trim().split(/\s+/).filter((word) => word && !word.startsWith("-") && !/^\d+$/.test(word));
  const working = workers.filter((row) => !row.gone && (row.lastEnd === null || row.working));
  if (ids.length === 0) return working;
  return workers.filter((row) => ids.some((id) => row.id === id || (id.length >= 4 && row.id.slice(-8).startsWith(id))));
}

/**
 * What the open turn's CLI is blocked on right now, named (D106): its own wait on a background
 * command or subagent it started (Claude's `TaskOutput`, Codex's agent wait) or a `brigadier
 * wait`. These calls have no row, so the live line would otherwise say "Thinking". `null` when
 * it waits on none of them.
 */
export function liveWait(view: ThreadView): StatusHead | null {
  const turn = view.turns.at(-1);
  if (!turn || turn.end !== null) return null;
  const from = turn.startedAtMs ?? 0;
  for (const entry of view.entries.toReversed()) {
    if (entry.parent || (entry.atMs ?? from) < from) continue;
    const { event } = entry;
    if ((event.type !== "toolCall" && event.type !== "command") || event.status !== "inProgress") continue;
    if (event.type === "toolCall" && event.name === "agents/wait") return subagentsWait(runningSubagentTasks(view, 0, null));
    if (event.type === "toolCall" && ["TaskOutput", "BashOutput"].includes(toolName(event.name))) {
      const input = inputOf(entry);
      const id = input.task_id ?? input.bash_id;
      const task = backgroundOf(view).find((each) => each.id === id);
      if (task) return taskWait(task, view.entries);
      continue;
    }
    const command = callCommand(entry);
    if (command && brigadierCalls(command).includes("wait")) return workersWait(waitedWorkers(command, view.workers ?? []), view.workers ?? []);
  }
  return null;
}

/**
 * What a block over still waits on, named when it is one thing (D106): its worker, background
 * command or subagent (`useBlockHold`'s holders). `null` when several hold it: the counts say it.
 */
export function heldWait(view: ThreadView, starts: readonly number[], startedAtMs: number, endedAtMs: number | null): StatusHead | null {
  const workers = workingWorkerRows(view.workers ?? [], view.turns, starts, endedAtMs);
  const commands = turnCommandTasks(view, view.turns, starts, endedAtMs);
  const subagents = runningSubagentTasks(view, startedAtMs, endedAtMs);
  if (workers.length + commands.length + subagents.length !== 1) return null;
  if (workers.length === 1) return workersWait(workers, view.workers ?? []);
  if (commands[0]) return taskWait(commands[0], view.entries);
  return subagentsWait(subagents);
}
