import { runsBrigadier, taskCommand } from "@/app/conversation/backgroundRows";
import { persistentApp } from "@/app/conversation/persistentApps";
import { taskEnd } from "@/app/conversation/backgroundTasks";
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
export const STRIP_TAIL_MS = 0;

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
  wait: boolean;
  /** The running tasks its Stop stops: itself, and a queued command's wait. */
  stops: string[];
};

/** The strip's rows, and what its closed line says first. */
export type StripLines = { lines: StripLine[]; waitingOn: string | null; stops?: string[]; stopStarts?: Record<string, number> };

const NONE: StripLines = { lines: [], waitingOn: null };

/** A shell loop that waits for something, and what runs after it: `until grep -q … f; do sleep 10; done; cat f`. */
const WAIT_LOOP = /^\s*(?:until|while)\s+([^]+?);\s*do\s+sleep\s+[\d.]+[smh]?\s*;?\s*done\s*(?:(?:;|&&)\s*([^]*))?$/;

/** The files a command writes what it prints to: `> f`, `>> f`, `tee f`; never a device (`/dev/null`). */
function writtenFiles(command: string): string[] {
  // Shell comments, input redirects and substitutions need a real shell parser. Retain
  // the independent wait on uncertainty rather than infer a writer from an argument.
  if (/[#$`\\<\n]/.test(command)) return [];
  const tokens = command.match(/'[^']*'|"[^"]*"|>>?|[;&|]+|[^\s'"<>;&|]+/g) ?? [];
  return tokens.flatMap((token, index) => {
    if (token !== ">" && token !== ">>" && token !== "tee") return [];
    if (token === "tee" && index !== 0 && !/^[;&|]+$/.test(tokens[index - 1]!)) return [];
    const operand = tokens[index + 1]?.replace(/^(['"])([^]*)\1$/, "$2");
    return operand && !operand.startsWith("-") && !operand.startsWith("&") && !operand.startsWith("/dev/") ? [operand] : [];
  });
}

/** Parse literal words only. Expansions, substitutions and shell operators are uncertain. */
function literalWords(text: string): string[] | null {
  if (/[`$\\|&;<>\n]/.test(text)) return null;
  const tokens = text.match(/'[^']*'|"[^"]*"|[^\s'"]+/g) ?? [];
  if (tokens.join(" ").replace(/\s+/g, " ") !== text.trim().replace(/\s+/g, " ")) return null;
  return tokens.map((token) => token.replace(/^(['"])([^]*)\1$/, "$2"));
}

/** Resolve only known literal absolute paths, or relative operands with an observed cwd. */
function pathOf(file: string, cwd: string | null): string | null {
  if (!file || /[$`*?{}~]/.test(file) || file.startsWith("/dev/")) return null;
  const absolute = file.startsWith("/") ? file : cwd?.startsWith("/") ? `${cwd}/${file}` : null;
  if (!absolute) return null;
  const parts: string[] = [];
  for (const part of absolute.split("/")) {
    if (part === "..") parts.pop();
    else if (part && part !== ".") parts.push(part);
  }
  return `/${parts.join("/")}`;
}

function commandCwd(task: BackgroundTask, entries: readonly FileEntry[]): string | null {
  const call = entries.find(({ event, parent }) => !parent && event.type === "command" && event.itemId === task.itemId)?.event;
  return call?.type === "command" ? call.cwd : null;
}

/** A narrow, proven file-read condition; more complex loops retain their own row. */
function watchedFile(condition: string, cwd: string | null): string | null {
  const words = literalWords(condition.replace(/\s+2>\/dev\/null\s*$/, ""));
  if (!words) return null;
  if (words[0] === "!") words.shift();
  if (words[0] === "test" && words[1] === "-s" && words.length === 3) return pathOf(words[2]!, cwd);
  if (!["grep", "rg"].includes(words[0] ?? "")) return null;
  let index = 1;
  while (/^-[qEFiInse]+$/.test(words[index] ?? "")) {
    // -e/-f have operands, so do not infer their meaning here.
    if (/[ef]/.test(words[index]!)) return null;
    index++;
  }
  if (words.length !== index + 2) return null;
  return pathOf(words[index + 1]!, cwd);
}

/** A continuation may fold only when it merely prints that same file. */
function onlyReads(rest: string, file: string, cwd: string | null): boolean {
  if (!rest.trim()) return true;
  const words = literalWords(rest);
  return words !== null && ["cat", "tail", "head", "wc"].includes(words[0] ?? "")
    && words.slice(1, -1).every((word) => /^-(?:n?\d+|[nqc]+)$/.test(word))
    && pathOf(words.at(-1) ?? "", cwd) === file;
}

type Candidate = { task: BackgroundTask; command: string | null; cwd: string | null };
type Wait = { target: BackgroundTask; after: string | null };

function waitOf(task: BackgroundTask, command: string | null, cwd: string | null, others: readonly Candidate[]): Wait | null {
  const loop = command?.match(WAIT_LOOP);
  if (!loop) return null;
  const [, condition = "", rest = ""] = loop;
  const file = watchedFile(condition, cwd);
  if (!file) return null;
  const named = others.filter((other) => other.task.id !== task.id && (
    pathOf(other.task.outputFile ?? "", other.cwd) === file
    || (other.command !== null && !/[$`]|\bcd\s/.test(other.command)
      && writtenFiles(other.command).some((name) => pathOf(name, other.cwd) === file))));
  if (named.length !== 1) return null;
  return { target: named[0]!.task, after: onlyReads(rest, file, cwd) ? null : rest.trim() };
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
  const all = tasks.filter((task) => task.kind !== "agent" && task.status === "running").map((task) => ({ task, command: taskCommand(task, entries), cwd: commandCwd(task, entries) }));
  const own = all.filter(({ command }) => !persistentApp(command ?? ""));
  const waits = new Map(own.map(({ task, command, cwd }) => [task.id, waitOf(task, command, cwd, own)] as const));
  const shown = own;
  void now;
  // A pure wait folds into what it waits on while that has its row; else it is a row of its own.
  const visible = shown.filter(({ task }) => {
    const wait = waits.get(task.id);
    return !wait || wait.after !== null || !shown.some((other) => other.task.id === wait.target.id);
  });
  const folded = (id: string) => !visible.some(({ task }) => task.id === id);
  if (shown.length === 0) return NONE;
  const lines = visible.toSorted((a, b) => a.task.startedAtMs - b.task.startedAtMs).map(({ task, command }): StripLine => {
    const wait = waits.get(task.id) ?? null;
    const queued = task.status === "running" && wait !== null && wait.target.status === "running";
    const end = taskEnd(task);
    const state: StripLineState = queued ? "queued" : end === null ? "running" : end.word === "finished" ? "done" : end.word;
    const word = queued
      ? `starts after ${commandName(wait.target)}`
      : end === null ? null
      : end.word === "finished" ? ("finished")
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
      wait: !queued && (runsBrigadier(command ?? "", "wait") || WAIT_LOOP.test(command ?? "") || /^\s*sleep\s+[\d.]+[smh]?\s*$/.test(command ?? "")),
      output: (task.kind === "shell" || task.kind === "terminal") && task.outputFile !== null,
      stops: task.status === "running" ? [task.id] : [],
    };
  });
  return { lines, waitingOn: lines.find((line) => line.waitedOn)?.name ?? lines.find((line) => runsBrigadier(line.command ?? "", "wait") || (line.state !== "queued" && WAIT_LOOP.test(line.command ?? "")))?.name.replace(/^wait(?:ing)?\s+(?:for|on)\s+/i, "") ?? null, stops: own.map(({ task }) => task.id), stopStarts: Object.fromEntries(own.map(({ task }) => [task.id, task.startedAtMs])) };
}

/**
 * A strip's closed line: what the session waits on first, then the counts, the first naming
 * them, zeros left out: "Waiting on Core tests · 2 checks running · 1 passed".
 */
export function checkWords({ lines, waitingOn }: StripLines): string {
  const [one, many, at] = ["check", "checks", "running"];
  const count = (test: (line: StripLine) => boolean) => lines.filter((line) => !line.wait && test(line)).length;
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
