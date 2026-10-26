import { runsBrigadier, taskCommand } from "@/app/conversation/backgroundRows";
import { taskEnd, taskShown } from "@/app/conversation/backgroundTasks";
import { commandWords } from "@/app/conversation/liveStatus";
import type { BackgroundTask, FileEntry } from "@/ipc/generated";

/**
 * The Checks strip above the composer: the session CLI's own background commands (tests, builds,
 * waits); its subagents have their own strip (`subagentsStrip`) and a worker's own tasks stay on
 * the worker's row. A wait that only watches another command
 * is no row of its own: that command says the session waits on it, but only when the wait names
 * that command's output for sure; any other wait is a row named by its own words.
 */

/** How long a strip stays after the last thing in it ended, so its end can be read. */
export const STRIP_TAIL_MS = 10_000;

export type StripLineState = "running" | "queued" | "done" | "failed" | "stopped";

/** One row of the Checks strip: a command, with the waits folded into it. */
export type StripLine = {
  /** The CLI's task id. */
  id: string;
  /** What it is: the CLI's description of it ("Run full core tests"). */
  name: string;
  /** What it last printed. */
  detail: string | null;
  state: StripLineState;
  /** Its state in words when it is not simply running: "passed", "failed · exit 1", "starts after Build". */
  word: string | null;
  startedAtMs: number;
  endedAtMs: number | null;
  /** A wait of the session's watches it now. */
  waitedOn: boolean;
  /** The command as it runs, for its tooltip. */
  command: string | null;
  /** Whether a click opens what it printed. */
  output: boolean;
  /** The running tasks its Stop stops: itself, and a queued command's wait. */
  stops: string[];
};

/** The strip's rows, and what its closed line says first. */
export type StripLines = { lines: StripLine[]; waitingOn: string | null };

const NONE: StripLines = { lines: [], waitingOn: null };

/** Whether a strip with these tasks shows at `now`: something runs, or the last end is a moment ago. */
function live(tasks: readonly BackgroundTask[], now: number): boolean {
  return tasks.some((task) => task.status === "running" || now - (task.endedAtMs ?? now) < STRIP_TAIL_MS);
}

/** A shell loop that waits for something, and what runs after it: `until grep -q … f; do sleep 10; done; cat f`. */
const WAIT_LOOP = /^\s*(?:until|while)\s+([^]+?);\s*do\s+sleep\s+[\d.]+[smh]?\s*;?\s*done\s*(?:(?:;|&&)\s*([^]*))?$/;

/** The files a command writes what it prints to: `> f`, `>> f`, `tee f`; never a device (`/dev/null`). */
function writtenFiles(command: string): string[] {
  return [...command.matchAll(/(?:\d?>>?|\btee(?:\s+-a)?)\s*(["']?)([^\s"';|&<>]+)\1/g)].map((match) => match[2]!).filter((file) => !file.startsWith("/dev/"));
}

/** Commands that only show what a command printed, as a wait's last step. */
const READERS = new Set(["cat", "tail", "head", "echo", "grep", "wc", "ls", "true", "printf"]);

/** Whether `rest` (what runs after a wait loop) only reads what was printed. */
const onlyReads = (rest: string) =>
  rest.split(/;|&&|\|\||\|/).map((part) => part.trim()).filter(Boolean).every((part) => READERS.has(part.split(/\s+/)[0] ?? ""));

type Wait = { target: BackgroundTask; after: string | null };

/**
 * What a wait waits on, when its loop names one other command's output for sure: that command's
 * output file, its `tasks/<id>.output`, or a file only its command writes to. `after` is what
 * runs after the loop when that is more than reading the output (a command queued behind it).
 */
function waitOf(task: BackgroundTask, command: string | null, others: readonly { task: BackgroundTask; command: string | null }[]): Wait | null {
  const loop = command?.match(WAIT_LOOP);
  if (!loop) return null;
  const [, condition = "", rest = ""] = loop;
  const named = others.filter((other) => other.task.id !== task.id && (
    (other.task.outputFile !== null && condition.includes(other.task.outputFile))
    || condition.includes(`/tasks/${other.task.id}.output`)
    || (other.command !== null && writtenFiles(other.command).some((file) => condition.includes(file)))));
  if (named.length !== 1) return null;
  return { target: named[0]!.task, after: rest.trim() && !onlyReads(rest) ? rest.trim() : null };
}

/** A command's name: the CLI's description of it, or the command itself when that is all it said. */
const commandName = (task: BackgroundTask) => (task.kind === "terminal" ? commandWords(task.label) : task.label.trim() || "Command");

/**
 * The session's own commands, oldest first: tests, builds and the like, each named by the CLI's
 * description and showing its last printed line. A wait that only watches one command it names
 * for sure is folded into it ("Session waits on this"); one with a command after it is that
 * command, queued until the first ends. `brigadier wait` belongs to the Workers strip.
 */
export function checkLines(tasks: readonly BackgroundTask[], entries: readonly FileEntry[], now: number): StripLines {
  const all = tasks.filter((task) => task.kind !== "agent").map((task) => ({ task, command: taskCommand(task, entries) }));
  const own = all.filter(({ command }) => !runsBrigadier(command ?? "", "wait"));
  const waits = new Map(own.map(({ task, command }) => [task.id, waitOf(task, command, own)] as const));
  const shown = own.filter(({ task }) => taskShown(task, now));
  // A pure wait folds into what it waits on while that has its row; else it is a row of its own.
  const visible = shown.filter(({ task }) => {
    const wait = waits.get(task.id);
    return !wait || wait.after !== null || !shown.some((other) => other.task.id === wait.target.id);
  });
  const folded = (id: string) => !visible.some(({ task }) => task.id === id);
  if (!live(visible.map(({ task }) => task), now) && !shown.some(({ task }) => task.status === "running")) return NONE;
  const lines = visible.toSorted((a, b) => a.task.startedAtMs - b.task.startedAtMs).map(({ task, command }): StripLine => {
    const wait = waits.get(task.id) ?? null;
    const queued = task.status === "running" && wait !== null && wait.target.status === "running";
    const end = taskEnd(task);
    const state: StripLineState = queued ? "queued" : end === null ? "running" : end.word === "finished" ? "done" : end.word;
    const word = queued
      ? `starts after ${commandName(wait.target)}`
      : end === null ? null
      : end.word === "finished" ? (end.exit === 0 ? "passed" : "finished")
      : end.word === "failed" && end.exit !== null ? `failed · exit ${end.exit}`
      : end.word;
    const watchers = own.filter((other) => other.task.status === "running" && folded(other.task.id) && waits.get(other.task.id)?.target.id === task.id);
    return {
      id: task.id,
      name: commandName(task),
      detail: task.lastLine?.trim() || null,
      state,
      word,
      startedAtMs: task.startedAtMs,
      endedAtMs: task.endedAtMs,
      waitedOn: task.status === "running" && watchers.length > 0,
      command,
      output: task.kind === "shell" && task.outputFile !== null,
      stops: task.status === "running" ? [task.id, ...watchers.map((other) => other.task.id)] : [],
    };
  });
  return { lines, waitingOn: lines.find((line) => line.waitedOn)?.name ?? null };
}

/**
 * A strip's closed line: what the session waits on first, then the counts, the first naming
 * them, zeros left out: "Waiting on Core tests · 2 checks running · 1 passed".
 */
export function checkWords({ lines, waitingOn }: StripLines): string {
  const [one, many, at] = ["check", "checks", "running"];
  const count = (test: (line: StripLine) => boolean) => lines.filter(test).length;
  const counts: [number, string][] = [
    [count((line) => line.state === "running"), at],
    [count((line) => line.state === "queued"), "queued"],
    [count((line) => line.state === "done" && line.word !== "finished"), "passed"],
    [count((line) => line.word === "finished"), "finished"],
    [count((line) => line.state === "failed"), "failed"],
    [count((line) => line.state === "stopped"), "stopped"],
  ];
  const words = counts.filter(([n]) => n > 0).map(([n, word], index) => (index === 0 ? `${n} ${n === 1 ? one : many} ${word}` : `${n} ${word}`));
  return [waitingOn && `Waiting on ${waitingOn}`, ...words].filter(Boolean).join(" · ");
}
