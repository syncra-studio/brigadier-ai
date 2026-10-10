import { taskWaitWords } from "@/app/conversation/taskActivity";
import type { Decision, DecisionWords, GateOwner, GateRole, MachineStepKind, MachineStepReason, Task } from "@/ipc/generated";

/**
 * The words of a worker's row in the thread ("Router: quota penalty · Landed · checked by
 * 1 review + 1 verify") and of the checks it opens to: plain functions of the board's
 * records, so every row says the same thing about the same task.
 */

/** How a row's state reads, and the colour family it takes besides its words. */
export type RowState = { word: string; tone: "live" | "done" | "warning" | "failed" | "quiet" };

/** A gate's owner as one key: `task:<id>`, `plan:<id>` or `phase:<run>:<phase>`. */
export function ownerKey(owner: GateOwner): string {
  switch (owner.type) {
    case "task":
      return `task:${owner.taskId}`;
    case "plan":
      return `plan:${owner.planId}`;
    case "phase":
      return `phase:${owner.runId}:${owner.phaseId}`;
  }
}

/** The checkers (reviewers, verifiers, judges) of the given owners, oldest first. */
export function checkersOf(tasks: Readonly<Record<string, Task>>, owners: readonly string[]): Task[] {
  const wanted = new Set(owners);
  return Object.values(tasks)
    .filter((task) => task.gateLink !== null && wanted.has(ownerKey(task.gateLink.owner)))
    .toSorted((a, b) => a.number - b.number);
}

const ROLE_WORDS: Record<GateRole, [one: string, many: string]> = {
  review: ["review", "reviews"],
  verify: ["verify", "verifies"],
  judge: ["judge", "judges"],
};

const ROLES: readonly GateRole[] = ["review", "verify", "judge"];

/** How many checks of each kind ran: "1 review + 2 verifies". Empty without any. */
export function checksCount(checkers: readonly Task[]): string {
  return ROLES.flatMap((role) => {
    const count = checkers.filter((task) => task.gateLink?.role === role).length;
    if (count === 0) return [];
    const [one, many] = ROLE_WORDS[role];
    return [`${count} ${count === 1 ? one : many}`];
  }).join(" + ");
}

/** A write task: its changes are checked and land on a branch. */
export function writes(task: Task): boolean {
  return task.kind === "implement" || task.kind === "merge";
}

/** A worker's state in a word or two, the same on its row wherever it shows. */
export function taskState(task: Task): RowState {
  const wait = task.quotaWait || task.state === "queued" || task.state === "blocked" || task.state === "paused"
    ? taskWaitWords(task)
    : null;
  if (wait) return { word: wait.split(":", 1)[0]!, tone: "quiet" };
  switch (task.state) {
    case "queued":
    case "starting":
      return { word: "Starting", tone: "live" };
    case "running":
    case "blocked":
      return { word: "Working", tone: "live" };
    case "paused":
      return { word: task.quotaWait ? "Waiting for quota" : "Paused", tone: "quiet" };
    case "reported":
      return { word: writes(task) ? "Finished" : "Reported", tone: "live" };
    case "takenOver":
      return { word: "In your terminal", tone: "warning" };
    case "landing":
      return { word: "Landing", tone: "live" };
    case "readyToLand":
      // In a run nobody waits for the user: the run's lead decides what a held change does.
      return task.run ? { word: "Held", tone: "warning" } : { word: "Ready to land", tone: "warning" };
    case "landed":
      return { word: "Landed", tone: "done" };
    case "done":
      return { word: "Done", tone: "done" };
    case "rejected":
      return { word: "Turned down", tone: "quiet" };
    case "stopped":
      return task.candidate && !task.landed
        ? { word: "Not landed", tone: "warning" }
        : { word: "Stopped", tone: "warning" };
    case "failed":
      return { word: "Failed", tone: "failed" };
  }
}

/** What follows a worker's state on its row: the checks an older task had ("checked by 1 review + 1 verify"). */
export function taskRowDetail(checkers: readonly Task[]): string {
  const counted = checksCount(checkers);
  return counted && `checked by ${counted}`;
}

/** What an older task's checker came to, from its report. */
export function checkResult(checker: Task): string {
  if (checker.state === "stopped") return "stopped";
  if (checker.state === "failed") return "failed";
  const report = checker.report;
  if (report?.verdict === "approve") return "passed";
  if (report?.verdict === "requestChanges") return "found problems";
  if (report?.checks === "failed") return "found problems";
  if (report?.checks === "notRun") return "couldn’t run its checks";
  if (checker.state === "done") return report?.checks === "passed" ? "passed" : "finished";
  return "checking";
}

/** A checker's role as its chip says it. */
export const ROLE_LABELS: Record<GateRole, string> = { review: "Review", verify: "Verify", judge: "Judge" };

/** Checkers grouped by the round they checked, oldest first. */
export function checkRounds(checkers: readonly Task[]): Task[][] {
  const rounds = new Map<string, Task[]>();
  for (const checker of checkers) {
    if (!checker.gateLink) continue;
    const key = `${ownerKey(checker.gateLink.owner)}#${checker.gateLink.round}`;
    const round = rounds.get(key);
    if (round) round.push(checker);
    else rounds.set(key, [checker]);
  }
  return [...rounds.values()];
}

/** A decision's line without the markdown it was written in. */
export function plainLine(text: string): string {
  return text.replace(/[`*]/g, "");
}

/**
 * A decision as the user reads it: the short words the daemon gives it when it reads the board
 * (the morning report's words, also for one recorded in an earlier version's longer ones), or
 * its own words for one that arrived since (recorded in the short words already).
 */
export function decisionWords(decision: Decision): DecisionWords {
  return decision.short ?? { what: decision.what, why: decision.why };
}

/** The decisions made about a task (landed, sent back, held), oldest first. */
export function taskDecisions(decisions: readonly Decision[], taskId: string): Decision[] {
  return decisions.filter((decision) => decision.source.type === "task" && decision.source.taskId === taskId);
}

/** Old verbose titles are shortened too; roles are metadata, not names. */
export function shortWorkerName(title: string, fallback = "Worker task"): string {
  const words = title.replace(/task-\d+|phase\s+\d+/gi, "").replace(/[`#*“”"():·—–]/g, " ")
    .replace(/\s+/g, " ").trim().split(" ").filter(Boolean);
  const short = words.slice(0, 4);
  while (short.length > 2 && /^(?:to|of|for|with|and|the|a|an)$/i.test(short.at(-1) ?? "")) short.pop();
  const name = short.join(" ") || fallback;
  return name.split(" ").length === 1 ? `${name} task` : name;
}

/** Unique in a chat, including old stored workers created before names were constrained. */
export function workerName(tasks: Readonly<Record<string, Task>>, task: Task, _depth = 0): string {
  const ordered = Object.values(tasks).toSorted((a, b) => a.number - b.number);
  if (!ordered.some((other) => other.id === task.id)) ordered.push(task);
  const used = new Set<string>();
  for (const other of ordered) {
    const fallback = other.kind === "review" ? "Review changes" : other.kind === "verify" ? "Check changes" : "Worker task";
    const base = shortWorkerName(other.title, fallback);
    let name = base;
    let ordinal = 2;
    while (used.has(name.toLowerCase())) name = `${base.split(" ").slice(0, 3).join(" ")} ${ordinal++}`;
    used.add(name.toLowerCase());
    if (other.id === task.id) return name;
  }
  return "Worker task";
}

/**
 * A worker's `task-N` standing on its own, with the title a line may already quote after it; one
 * inside a branch, path or longer word (`brigadier/x/task-13-fix`, `docs/task-13.md`) is left be,
 * as the daemon's `named` does.
 */
const TASK_REF = /(?<![\p{L}\p{N}_/-])task-(\d+)(?![\p{L}\p{N}_/-]|\.\w)(\s+[“"][^”"]*[”"])?/gu;

/**
 * A line with each `task-N` it names given as that worker's name in quotes ("Landed task-41
 * “Findings”" reads "Landed “Findings”"); a number the session has no worker for stays.
 */
export function namedTasks(text: string, tasks: Readonly<Record<string, Task>>, depth = 0): string {
  if (!text.includes("task-")) return text;
  const byNumber = new Map(Object.values(tasks).map((task) => [task.number, task]));
  return text.replace(TASK_REF, (whole, number: string, quoted: string | undefined) => {
    const task = byNumber.get(Number(number));
    if (!task) return whole;
    return quoted ? quoted.trimStart() : `“${workerName(tasks, task, depth)}”`;
  });
}

/**
 * A row about the machine ("Paused cargo test to let the Mac cool down"); `machine` is how
 * the OS's computer is called ("the Mac").
 */
export function machineWords(kind: MachineStepKind, command: string | null, machine: string, reason: MachineStepReason = "heat"): string {
  switch (kind) {
    case "waitingToCool":
      return reason === "memory" ? "Waiting for memory to free up" : `Waiting for ${machine} to cool down`;
    case "waitingForBuild":
      return "Waiting for another build to finish";
    case "paused":
      return `Paused ${command ?? "a build"} to let ${machine} cool down`;
    case "resumed":
      return `Resumed ${command ?? "a build"}`;
  }
}
