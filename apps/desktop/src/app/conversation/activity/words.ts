import { namedTasks, workerName } from "@/app/conversation/rowWords";
import type { TranscriptItem } from "@/components/transcript/transcript";
import type { ItemStatus, OrchestratorStepKind, Task } from "@/ipc/generated";

/**
 * The one vocabulary of the thread's work (THREAD-UX-PLAN.md §3.2): what a step of the lead or
 * of a worker is called on its own row ("Read RequestBlock.tsx", "Ran pnpm test") and how a run
 * of steps sums up ("Read files, ran commands"). The main thread and a worker's thread describe
 * the same call the same way, because both go through [`stepWords`].
 */

/** The kinds of work a group sums up, in the order of the plan's table. */
export type WorkKind = "read" | "edit" | "run" | "code" | "web" | "memory" | "checks" | "preview" | "other";

/** A call as both threads know it: the tool's name, its one-line detail and how it stands. */
export type StepCall = { name: string; detail: string | null; status: ItemStatus };

/** What a step is called: its kind, its words while it runs and once it ran. */
export type StepWords = {
  kind: WorkKind;
  doing: string;
  done: string;
  /** What it acts on, as both its words end with it (a file, a command, a pattern, a host): told a
   * step fainter than its verb. */
  object?: string;
  /** For `other`: the summary's words for it ("Asked for a review"). */
  phrase?: string;
  /** It reaches the network (a web page, `curl`, `git fetch`). */
  web?: boolean;
};

export type ToolStep = Extract<OrchestratorStepKind, { type: "tool" }>;

/** Providers wrap the same tool in different namespaces; those are never UI copy. */
export function toolName(name: string): string {
  return (
    name
      .split(/__|[./:]/)
      .filter(Boolean)
      .at(-1) ?? ""
  );
}

/**
 * Calls on Brigadier's own plumbing, never a row: what came of them shows elsewhere (a worker's
 * team sentence, a card, the Brain, the merge the user asked for).
 */
const PLUMBING = new Set([
  "ToolSearch",
  "tool_search",
  "read_report",
  // Its authored step says what was read ("Read notes.md"), or the team sentence covers a diff.
  "read_artifact",
  "finish_session",
  "note_for_user",
  "remember",
  "route_follow_up",
  "list_tasks",
  "land_phase",
  "delegate_task",
  "spawn_agent",
  "message_worker",
  "answer_worker",
  "stop_worker",
  "start_verifier",
  "ask_user",
  "propose_merge",
  "submit_report",
]);

/** Whether a call is plumbing, shown by what came of it rather than as a row. */
export function isPlumbing(name: string): boolean {
  return PLUMBING.has(toolName(name));
}

/** What `other` calls Brigadier knows are called; anything else is "Used {name}". */
const OTHER: Record<string, [doing: string, done: string]> = {
  search_transcript: ["Searching earlier messages", "Searched earlier messages"],
  plan_phases: ["Planning the work", "Planned the work"],
  propose_overnight: ["Planning the work", "Planned the work"],
  approve_outline: ["Reviewing an outline", "Reviewed an outline"],
  settle_step: ["Settling a phase", "Settled a phase"],
  end_run: ["Ending the run", "Ended the run"],
  review_code: ["Asking for a review", "Asked for a review"],
  review_plan: ["Asking for a review", "Asked for a review"],
  request_approval: ["Asking for approval", "Asked for approval"],
  ask_orchestrator: ["Asking the lead", "Asked the lead"],
  submit_outline: ["Sending an outline", "Sent an outline"],
  imagegen: ["Creating an image", "Created an image"],
};

const READS = new Set(["Read", "read", "read_file", "read_artifact"]);
const EDITS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"]);
const COMMANDS = new Set(["Bash", "run", "run_unsandboxed", "exec_command", "shell"]);
const CODE_SEARCH = new Set(["Grep", "code_search", "code_refs"]);
const LISTINGS = new Set(["Glob", "LS", "project_map"]);
const WEB_SEARCH = new Set(["WebSearch", "web_search"]);
const WEB_FETCH = new Set(["WebFetch", "web_fetch"]);
const PREVIEWS: Record<string, [doing: string, done: string]> = {
  start_preview: ["Starting a preview", "Started a preview"],
  stop_preview: ["Stopping a preview", "Stopped a preview"],
  preview_log: ["Reading the preview’s output", "Read the preview’s output"],
};

const READERS = new Set(["cat", "head", "tail", "nl", "less", "more", "bat", "wc", "sed"]);
const LISTERS = new Set(["ls", "find", "tree", "fd"]);
const SEARCHERS = new Set(["rg", "grep", "ag", "ack"]);
const NETWORK = new Set(["curl", "wget", "http", "ping", "ssh", "scp", "nc", "dig"]);
const NETWORK_GIT = new Set(["fetch", "pull", "push", "clone", "ls-remote"]);

/**
 * The command a shell wrapper runs, without moving into its folder first:
 * `/bin/zsh -c 'wc -l notes.py'` and `cd /tmp/wt && wc -l notes.py` → `wc -l notes.py`.
 */
export function unwrapCommand(command: string): string {
  const wrapped = /^(?:\/\S*\/)?(?:ba|z)?sh\s+-l?c\s+(['"])([\s\S]*)\1$/.exec(command.trim());
  return (wrapped?.[2] ?? command).trim().replace(/^(?:cd\s+(?:'[^']*'|"[^"]*"|\S+)\s*&&\s*)+/, "");
}

export function basename(path: string): string {
  return path.replace(/\/+$/, "").split("/").pop() || path;
}

/** A simple command's words, quotes removed: `grep -n "def test_" tests` → 4 words. */
function wordsOf(command: string): string[] {
  return [...command.matchAll(/'([^']*)'|"([^"]*)"|(\S+)/g)].map((match) => match[1] ?? match[2] ?? match[3] ?? "");
}

/** The last path-like argument, if any (`sed -n 1,80p notes.py` → notes.py). */
function lastPath(words: readonly string[]): string | null {
  const last = words
    .slice(1)
    .filter((word) => !word.startsWith("-") && !/^\d/.test(word))
    .at(-1);
  return last ? basename(last) : null;
}

/** "Read notes.py" / "Reading notes.py". */
const read = (what: string): StepWords => ({ kind: "read", doing: `Reading ${what}`, done: `Read ${what}`, object: what });

const listed = (folder: string | null): StepWords =>
  folder
    ? { kind: "code", doing: `Listing files in ${folder}`, done: `Listed files in ${folder}`, object: folder }
    : { kind: "code", doing: "Listing files", done: "Listed files" };

const searchedCode = (pattern: string | null): StepWords => {
  if (!pattern) return { kind: "code", doing: "Searching code", done: "Searched code" };
  const object = `“${pattern}”`;
  return { kind: "code", doing: `Searching code for ${object}`, done: `Searched code for ${object}`, object };
};

/** A command, told as what it does: a read, a listing, a search, or a run of its first line. */
function commandWords(raw: string, kind: "run" | "checks" = "run"): StepWords {
  const command = unwrapCommand(raw);
  const [head = command, ...rest] = command.trim().split("\n");
  const firstLine = rest.length > 0 ? `${head} …` : head;
  const words = wordsOf(command);
  const program = basename(words[0] ?? "");
  const web = NETWORK.has(program) || (program === "git" && NETWORK_GIT.has(words[1] ?? ""));
  const ran: StepWords = { kind, doing: `Running ${firstLine}`, done: `Ran ${firstLine}`, object: firstLine, ...(web && { web }) };
  // A compound command, or a check, is told as what it runs.
  if (kind === "checks" || /[;&|<>`$(]/.test(command.replace(/\s\|\|\s.*$/, ""))) return ran;
  if (program === "rg" && words.includes("--files")) return listed(null);
  if (READERS.has(program)) {
    const file = lastPath(words);
    return file ? read(file) : ran;
  }
  if (LISTERS.has(program)) return listed(lastPath(words));
  if (SEARCHERS.has(program)) return searchedCode(words.slice(1).find((word) => !word.startsWith("-")) ?? null);
  return ran;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/** A tool's own words for `other`: what Brigadier calls it, else "Used {name}". */
function otherWords(short: string): StepWords {
  const known = OTHER[short];
  if (known) return { kind: "other", doing: known[0], done: known[1], phrase: known[1] };
  const action = short
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .toLowerCase()
    .trim();
  const name = action && action !== "tool" ? action : "a tool";
  return { kind: "other", doing: `Using ${name}`, done: `Used ${name}`, phrase: `Used ${name}` };
}

/**
 * A step's words. `detail` is the call's one-line detail as the daemon records it for the lead
 * (the query, pattern, path, URL, name or command) and as [`detailOf`] reads it from a worker's
 * call; `tasks` names the workers a detail mentions.
 */
export function stepWords(call: StepCall, tasks: Readonly<Record<string, Task>> = {}): StepWords {
  const short = toolName(call.name);
  const detail = call.detail?.trim() ? namedTasks(call.detail.trim(), tasks) : null;
  if (READS.has(short)) return detail ? read(basename(detail)) : { kind: "read", doing: "Reading a file", done: "Read a file" };
  if (EDITS.has(short)) {
    if (!detail) return { kind: "edit", doing: "Editing a file", done: "Edited a file" };
    const [path, more] = detail.split(/ (and \d+ more)$/);
    const what = `${basename(path ?? detail)}${more ? ` ${more}` : ""}`;
    return { kind: "edit", doing: `Editing ${what}`, done: `Edited ${what}`, object: what };
  }
  if (COMMANDS.has(short)) {
    return detail ? commandWords(detail) : { kind: "run", doing: "Running a command", done: "Ran a command" };
  }
  if (short === "run_check") {
    return detail ? commandWords(detail, "checks") : { kind: "checks", doing: "Running checks", done: "Ran checks" };
  }
  if (CODE_SEARCH.has(short)) return searchedCode(detail);
  if (LISTINGS.has(short)) return listed(detail ? basename(detail) : null);
  if (short === "query_brain") {
    if (!detail) return { kind: "memory", doing: "Checking project memory", done: "Checked project memory" };
    const object = `“${detail}”`;
    return { kind: "memory", doing: `Checking project memory for ${object}`, done: `Checked project memory for ${object}`, object };
  }
  if (WEB_SEARCH.has(short)) {
    if (!detail) return { kind: "web", doing: "Searching the web", done: "Searched the web", web: true };
    return { kind: "web", doing: `Searching the web for ${detail}`, done: `Searched the web for ${detail}`, object: detail, web: true };
  }
  if (WEB_FETCH.has(short)) {
    if (!detail) return { kind: "web", doing: "Reading a web page", done: "Read a web page", web: true };
    const page = hostOf(detail);
    return { kind: "web", doing: `Reading ${page}`, done: `Read ${page}`, object: page, web: true };
  }
  const preview = PREVIEWS[short];
  if (preview) return { kind: "preview", doing: preview[0], done: preview[1] };
  return otherWords(short);
}

/** The keys a call's detail comes from, in the daemon's order (`conversation.rs`, tool steps). */
const DETAIL_KEYS = ["query", "pattern", "file_path", "path", "url", "title", "task", "name", "command"];

/** A worker's call's one-line detail, read from its input the way the daemon reads the lead's. */
export function detailOf(input: string | null): string | null {
  if (!input) return null;
  try {
    const value: unknown = JSON.parse(input);
    if (!value || typeof value !== "object") return null;
    const args = value as Record<string, unknown>;
    for (const key of DETAIL_KEYS) {
      const found = args[key];
      if (typeof found === "string") return found.slice(0, 240);
    }
    return null;
  } catch {
    return null;
  }
}

/** An edit's lines out and in, in one file. */
export type Hunk = { path: string; removed: string[]; added: string[] };

/** A call's input as its arguments, when it is a JSON object. */
export function parsedInput(input: string | null): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(input ?? "");
    return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

const text = (value: unknown): string => (typeof value === "string" ? value : "");
const linesOf = (value: string): string[] => (value ? value.replace(/\n$/, "").split("\n") : []);

/** An edit's changes, read from its call: Edit's old and new text, MultiEdit's edits, Write's content. */
export function editHunks(name: string, input: string | null): Hunk[] {
  const args = parsedInput(input);
  if (!args) return [];
  const path = text(args["file_path"]) || text(args["path"]);
  switch (toolName(name)) {
    case "Edit":
      return [{ path, removed: linesOf(text(args["old_string"])), added: linesOf(text(args["new_string"])) }];
    case "MultiEdit": {
      const edits = Array.isArray(args["edits"]) ? (args["edits"] as Record<string, unknown>[]) : [];
      return edits.map((edit) => ({ path, removed: linesOf(text(edit["old_string"])), added: linesOf(text(edit["new_string"])) }));
    }
    case "Write":
      return [{ path, removed: [], added: linesOf(text(args["content"])) }];
    default:
      return [];
  }
}

/** An edit's lines as diff text: `-` out, then `+` in. */
export function hunkDiff(hunk: Hunk): string {
  return [...hunk.removed.map((line) => `-${line}`), ...hunk.added.map((line) => `+${line}`)].join("\n");
}

/**
 * The daemon's text of an edit's changes (`getThreadItem`): each file's path on a line of its
 * own, then its diff lines. A line that starts a new file is one that isn't a diff line.
 */
export function fileDiffs(changes: string): { path: string; diff: string }[] {
  const files: { path: string; diff: string[] }[] = [];
  for (const line of changes.split("\n")) {
    const last = files.at(-1);
    if (last && /^[-+@ \\]|^$/.test(line)) last.diff.push(line);
    else files.push({ path: line, diff: [] });
  }
  return files.map((file) => ({ path: file.path, diff: file.diff.join("\n") }));
}

/** A worker's file changes as the lead's `apply_patch` step names them. */
export function changesDetail(paths: readonly string[]): string | null {
  const [first] = paths;
  if (first === undefined) return null;
  return paths.length === 1 ? first : `${first} and ${paths.length - 1} more`;
}

/** Only suppress a raw call when its authored result actually exists in this turn. */
const OWN_RESULT: Record<string, string> = {
  delegate_task: "created",
  start_verifier: "created",
  message_worker: "messaged",
  answer_worker: "answered",
  read_report: "readReport",
  read_artifact: "readArtifact",
  note_for_user: "decided",
  land_phase: "landed",
  WebSearch: "searchedWeb",
  web_search: "searchedWeb",
  WebFetch: "readPage",
  web_fetch: "readPage",
};

export function toolHasOwnResult(
  kind: ToolStep,
  results?: readonly {
    kind: { type: string };
    position: number;
    requestId: string | null;
  }[],
  requestId?: string | null,
  position = 0,
): boolean {
  const result = OWN_RESULT[toolName(kind.name)];
  if (kind.status !== "completed" || !result) return false;
  // The board may be replayed one event at a time. Keep the call until its result arrives.
  const nextCall =
    results?.find(
      (step) => step.kind.type === "tool" && step.position > position,
    )?.position ?? Number.POSITIVE_INFINITY;
  return !!results?.some(
    (step) =>
      step.requestId === requestId &&
      step.kind.type === result &&
      step.position >= position &&
      step.position < nextCall,
  );
}

/**
 * A step's row: its words for how it stands, and how it ended when that wasn't well ("Ran pnpm
 * test — failed (exit 1)"). `exit` is a command's exit code, when it says.
 */
export function stepLabel(words: StepWords, status: ItemStatus, exit: number | null = null): string {
  if (status === "inProgress") return words.doing;
  if (status === "declined") return `${words.done} — stopped`;
  if (exit !== null && exit !== 0) return `${words.done} — failed (exit ${exit})`;
  if (status === "failed") return `${words.done} — failed`;
  return words.done;
}

/**
 * A row's words in two tones: its verb, then what it acts on (fainter), then how it ended.
 * Words with no object are all verb.
 */
export function labelParts(words: StepWords, label: string): { verb: string; object: string; rest: string } {
  const at = words.object ? label.indexOf(words.object, 1) : -1;
  if (!words.object || at < 0) return { verb: label, object: "", rest: "" };
  return { verb: label.slice(0, at), object: words.object, rest: label.slice(at + words.object.length) };
}

/** A lead's tool step's words, with its workers named. */
export function toolStepWords(kind: ToolStep, tasks: Readonly<Record<string, Task>> = {}): StepWords {
  const worker = kind.detail ? tasks[kind.detail] : undefined;
  return stepWords({ ...kind, detail: worker ? workerName(tasks, worker) : kind.detail }, tasks);
}

/** How a kind leads a summary, for one step and for several. */
const LEADING: Record<Exclude<WorkKind, "other">, [one: string, many: string]> = {
  read: ["Read a file", "Read files"],
  edit: ["Edited a file", "Edited files"],
  run: ["Ran a command", "Ran commands"],
  code: ["Searched code", "Searched code"],
  web: ["Searched the web", "Searched the web"],
  memory: ["Checked project memory", "Checked project memory"],
  checks: ["Ran checks", "Ran checks"],
  preview: ["Started a preview", "Started a preview"],
};

/** "A, B and C". */
export function joinWords(parts: readonly string[]): string {
  if (parts.length <= 1) return parts[0] ?? "";
  return `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`;
}

/**
 * A run of steps in one sentence, its kinds in the order they first came: "Read files, ran
 * commands and searched code". Each `other` tool counts as a kind of its own.
 */
export function summarize(steps: readonly StepWords[]): string {
  const counts = new Map<string, { words: StepWords; count: number }>();
  for (const words of steps) {
    const key = words.kind === "other" ? `other:${words.phrase ?? words.done}` : words.kind;
    const known = counts.get(key);
    counts.set(key, { words, count: (known?.count ?? 0) + 1 });
  }
  const parts = [...counts.values()].map(({ words, count }) =>
    words.kind === "other" ? (words.phrase ?? words.done) : LEADING[words.kind][count === 1 ? 0 : 1],
  );
  const following = parts.map((part, index) => (index === 0 ? part : part.charAt(0).toLowerCase() + part.slice(1)));
  return joinWords(following);
}

/** A worker's action, as its transcript keeps it. */
export type ActionItem = Extract<TranscriptItem, { kind: "command" | "tool" | "files" | "image" }>;

/** A worker's action as the call the lead's step would record for it. */
export function itemCall(item: ActionItem): StepCall {
  switch (item.kind) {
    case "command":
      return { name: "shell", detail: item.command, status: item.status };
    case "tool":
      return { name: item.name, detail: detailOf(item.input), status: item.status };
    case "files":
      return { name: "apply_patch", detail: changesDetail(item.changes.map((change) => change.path)), status: item.status };
    case "image":
      return { name: "imagegen", detail: item.prompt, status: item.status };
  }
}

/** What a worker's live command or call is doing, in the thread's words ("Reading notes.py"). */
export function liveActivity(call: { command: string } | { name: string; input: string | null }): string {
  return stepWords("command" in call
    ? { name: "shell", detail: call.command, status: "inProgress" }
    : { name: call.name, detail: detailOf(call.input), status: "inProgress" }).doing;
}
