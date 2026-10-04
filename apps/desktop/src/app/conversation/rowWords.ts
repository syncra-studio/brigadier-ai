import type { Decision, Gate, GateOwner, GateRole, Task } from "@/ipc/generated";

/**
 * The words of a worker's row in the thread ("Router: quota penalty · Landed · checked by
 * 1 review + 1 verify · 1 fix") and of the checks it opens to: plain functions of the board's
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
  switch (task.state) {
    case "queued":
    case "starting":
      return { word: "Starting", tone: "live" };
    case "running":
    case "blocked":
      return { word: task.fixRounds > 0 ? `Fixing (${task.fixRounds} of 2)` : "Working", tone: "live" };
    case "paused":
      return { word: task.quotaWait ? "Waiting for quota" : "Paused", tone: "quiet" };
    case "reported":
      return { word: writes(task) ? "Finished" : "Reported", tone: "live" };
    case "reviewing":
      return { word: "Checking", tone: "live" };
    case "awaitingApproval":
      return { word: "Waiting for you", tone: "warning" };
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

/** What follows a worker's state on its row: its checks and fix rounds ("checked by 1 review + 1 verify · 1 fix"). */
export function taskRowDetail(task: Task, checkers: readonly Task[]): string {
  const counted = checksCount(checkers);
  // One entry per time it was sent back with findings; the round counter resets once it lands.
  const fixes = task.fixes.length;
  const parts = [
    counted && (task.state === "reviewing" ? counted : `checked by ${counted}`),
    fixes > 0 && `${fixes} ${fixes === 1 ? "fix" : "fixes"}`,
  ];
  return parts.filter(Boolean).join(" · ");
}

/** What a checker came to: from its round on the owner while the owner keeps it, else from its report. */
export function checkResult(checker: Task, gate: Gate | null): string {
  const member =
    gate && checker.gateLink && gate.round === checker.gateLink.round
      ? gate.members.find((candidate) => candidate.taskId === checker.id)
      : undefined;
  switch (member?.result?.type) {
    case "passed":
      return "passed";
    case "failed":
      return "found problems";
    case "unverified":
      return "couldn’t verify";
    case "noResult":
      return "no result";
    default:
      break;
  }
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

/** The decisions made about a task (landed, sent back, held), oldest first. */
export function taskDecisions(decisions: readonly Decision[], taskId: string): Decision[] {
  return decisions.filter((decision) => decision.source.type === "task" && decision.source.taskId === taskId);
}

/**
 * What a worker is called wherever the user reads it: its title, or for a check of another
 * worker "Review of", "Second review of" or "Check of" and that worker's name. Its `task-N`
 * stays in records and prompts only; a title that names one gets the worker's name instead.
 */
export function workerName(tasks: Readonly<Record<string, Task>>, task: Task, depth = 0): string {
  const subject = task.subject ? tasks[task.subject] : undefined;
  if (subject && depth < 2) {
    const of = workerName(tasks, subject, depth + 1);
    if (task.kind === "review") return `${/^second review/i.test(task.title) ? "Second review" : "Review"} of ${of}`;
    if (task.kind === "verify") return `Check of ${of}`;
  }
  return depth < 2 ? namedTasks(task.title, tasks, depth + 1) : task.title;
}

/** A worker's `task-N`, with the title a line may already quote after it. */
const TASK_REF = /\btask-(\d+)(\s+[“"][^”"]*[”"])?/g;

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
