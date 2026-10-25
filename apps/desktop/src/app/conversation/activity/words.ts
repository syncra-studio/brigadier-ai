import { namedTasks, workerName } from "@/app/conversation/rowWords";
import type { TranscriptItem } from "@/components/conversation/transcript/transcript";
import type { Approval, ItemStatus, OrchestratorStepKind, Task } from "@/ipc/generated";

/**
 * The one vocabulary of the thread's work (THREAD-UX-PLAN.md §3.2): what a step of the lead or
 * of a worker is called on its own row ("Read RequestBlock.tsx", "Ran pnpm test") and how a run
 * of steps sums up ("Read files, ran commands"). The main thread and a worker's thread describe
 * the same call the same way, because both go through [`stepWords`].
 */

/** The kinds of work a group sums up, in the order of the plan's table. */
export type WorkKind = "read" | "edit" | "run" | "code" | "web" | "message" | "other";

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
  /** For `other`: the summary's words for several of it ("Ran agents"), where they differ. */
  many?: string;
  /** It reads the web (a search, a page, a plain GET of a public page): drawn with a globe. */
  web?: boolean;
  /** It waits for other sessions to reply. */
  waiting?: boolean;
  /** It lists a folder's files (drawn with a folder, where a search has a magnifier). */
  list?: boolean;
  /** The files an edit changed, when known: a summary counts files, not calls ("Edited files"). */
  paths?: readonly string[];
  /** A tool Brigadier has no words of its own for ("Used search issues"): its repeats fold into one row (#101). */
  unnamed?: boolean;
  /** For `unnamed`: what it was given (its detail), said on its row inside that fold ("checkout tax"). */
  given?: string;
};

export type ToolStep = Extract<OrchestratorStepKind, { type: "tool" }> & {
  /** Still running in the background after its turn ended (the thread's own steps only, D44). */
  background?: boolean;
};

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
 * Calls of the CLIs' own plumbing, never a row: what came of them shows elsewhere (the tools a
 * search loaded, the question card and plan card of the CLI's own question tool and plan,
 * harness D7), and Codex's wait for its own agents, whose rows say when each finished (D44).
 */
const PLUMBING = new Set(["ToolSearch", "tool_search", "AskUserQuestion", "request_user_input", "ExitPlanMode"]);

/** Whether a call is plumbing, shown by what came of it rather than as a row. */
export function isPlumbing(name: string): boolean {
  return PLUMBING.has(toolName(name)) || name === "agents/wait";
}

/** A subagent's start: Claude's `Agent` (once `Task`), Codex's `spawnAgent`. */
const AGENTS = new Set(["Agent", "Task", "spawnAgent", "spawn_agent"]);

/**
 * What a subagent or skill call is given on its row (B1): a Claude agent's description, a Codex
 * agent's prompt's first line, a skill's name; `undefined` for any other call.
 */
export function agentDetail(name: string, input: string | null): string | null | undefined {
  const short = toolName(name);
  if (!AGENTS.has(short) && short !== "Skill") return undefined;
  const args = parsedInput(input);
  const given = args ? (short === "Skill" ? args["skill"] : (args["description"] ?? args["prompt"])) : input;
  if (typeof given !== "string") return null;
  const line = given.trim().split("\n")[0]?.trim() ?? "";
  return line ? [...line].slice(0, 240).join("") : null;
}

/** What `other` calls Brigadier knows are called; anything else is "Used {name}". */
const OTHER: Record<string, [doing: string, done: string]> = {
  imagegen: ["Creating an image", "Created an image"],
};

const READS = new Set(["Read", "read", "read_file"]);
const EDITS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"]);
const COMMANDS = new Set(["Bash", "exec_command", "shell"]);
const CODE_SEARCH = new Set(["Grep"]);
const LISTINGS = new Set(["Glob", "LS"]);
const WEB_SEARCH = new Set(["WebSearch", "web_search"]);
const WEB_FETCH = new Set(["WebFetch", "web_fetch"]);

const READERS = new Set(["cat", "head", "tail", "nl", "less", "more", "bat", "wc", "sed"]);
const LISTERS = new Set(["ls", "find", "tree", "fd"]);
const SEARCHERS = new Set(["rg", "grep", "ag", "ack"]);

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
    ? { kind: "code", doing: `Listing files in ${folder}`, done: `Listed files in ${folder}`, object: folder, list: true }
    : { kind: "code", doing: "Listing files", done: "Listed files", list: true };

/** Options of `curl` and `wget` that send or change something: such a call is no plain page read. */
const SENDS = /^(?:-X|--request|-d|--data\S*|-F|--form\S*|-T|--upload-file|--post-data|--post-file|--method|--body-data|--body-file)$/;

/** Whether a URL names a host on the internet, not this machine or the local network. */
function publicHost(url: string): boolean {
  try {
    const { protocol, hostname } = new URL(url);
    if (protocol !== "http:" && protocol !== "https:") return false;
    if (!hostname.includes(".") || hostname.endsWith(".local") || hostname.endsWith(".internal")) return false;
    return !/^(?:127\.|10\.|192\.168\.|172\.(?:1[6-9]|2\d|3[01])\.|169\.254\.|0\.)/.test(hostname) && !hostname.startsWith("[");
  } catch {
    return false;
  }
}

/** `curl https://nodejs.org/dist/index.json`: a GET of a public page, which reads as a web search. */
function readsTheWeb(program: string, words: readonly string[]): boolean {
  if (program !== "curl" && program !== "wget") return false;
  if (words.some((word, index) => index > 0 && SENDS.test(word.replace(/=.*$/, "")))) return false;
  const urls = words.slice(1).filter((word) => /^https?:\/\//.test(word));
  return urls.length > 0 && urls.every(publicHost);
}

/** "Searched for “add” in src" / "Searching for “add”". */
const searchedCode = (pattern: string | null, path: string | null = null): StepWords => {
  if (!pattern) return { kind: "code", doing: "Searching files", done: "Searched files" };
  const object = `“${pattern}”`;
  const where = path ? ` in ${path}` : "";
  return { kind: "code", doing: `Searching for ${object}${where}`, done: `Searched for ${object}${where}`, object };
};

/**
 * The `brigadier <sub>` commands `command` runs, in order, each as a command of its own (after
 * `start &&` too), never words inside an argument (`core::manager::cli_jobs::runs_brigadier`
 * reads it the same).
 */
export function brigadierCalls(command: string): string[] {
  const tokens = unwrapCommand(command).match(/(?:'[^']*'|"(?:\\.|[^"\\])*"|\\[^\n]|[^\s;&|()'"\\])+|[;&|()\n]/g) ?? [];
  const subs: string[] = [];
  let start = true;
  for (const [index, token] of tokens.entries()) {
    if (/^[;&|()\n]$/.test(token)) {
      start = true;
    } else if (start) {
      if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(token)) continue;
      const sub = tokens[index + 1];
      if (/^(?:\S*\/)?brigadier$/.test(token) && sub !== undefined) subs.push(sub);
      start = false;
    }
  }
  return subs;
}

/**
 * What the thread's own `brigadier` calls are called (D38, D47: under the hood, in plain words);
 * `wait` and `start` have no row: the block's "Waiting for a worker" line and the worker's own
 * row say them.
 */
const BRIGADIER: Record<string, [doing: string, done: string]> = {
  sessions: ["Checking the sessions", "Checked the sessions"],
  models: ["Checking the models", "Checked the models"],
  workers: ["Checking the workers", "Checked the workers"],
  show: ["Checking a worker", "Checked a worker"],
  read: ["Reading a worker's messages", "Read a worker's messages"],
  why: ["Checking why a line changed", "Checked why a line changed"],
  send: ["Messaging workers", "Messaged workers"],
  close: ["Closing a worker", "Closed a worker"],
  queue: ["Queueing a message", "Queued a message"],
  context: ["Reading the brigadier context", "Read the brigadier context"],
  ask: ["Asking questions", "Asked questions"],
  preview: ["Working on the preview", "Worked on the preview"],
  checkpoint: ["Writing the checkpoint", "Wrote the checkpoint"],
};

/** The words of a command that runs `brigadier <sub>` first, by that sub. */
function brigadierWords(sub: string, command: string): StepWords {
  const words = wordsOf(command);
  const ledger = words[words.indexOf("ledger") + 1];
  const [doing, done] =
    sub === "ledger"
      ? ledger === "add" || ledger === "close"
        ? ["Updating the ledger", "Updated the ledger"]
        : ledger === "check"
          ? ["Checking against the ledger", "Checked against the ledger"]
          : ["Reading the ledger", "Read the ledger"]
      : (BRIGADIER[sub] ?? ["Using Brigadier", "Used Brigadier"]);
  return { kind: "other", doing, done, phrase: done };
}

/** A call that is no row of its own: the thread's `brigadier wait` and `brigadier start` (D47). */
export function brigadierUnshown(command: string): boolean {
  const [sub] = brigadierCalls(command);
  return sub === "wait" || sub === "start";
}

/** A command, told as what it does: a read, a listing, a search, or a run of its first line. */
function commandWords(raw: string): StepWords {
  const [brigadier] = brigadierCalls(raw);
  if (brigadier !== undefined) return brigadierWords(brigadier, unwrapCommand(raw));
  const command = unwrapCommand(raw);
  const [head = command, ...rest] = command.trim().split("\n");
  const firstLine = rest.length > 0 ? `${head} …` : head;
  const words = wordsOf(command);
  const program = basename(words[0] ?? "");
  // Any other command has a terminal, also one that reaches the network (`curl … | python3`, `git fetch`).
  const ran: StepWords = { kind: "run", doing: `Running ${firstLine}`, done: `Ran ${firstLine}`, object: firstLine };
  // A compound command is told as what it runs.
  if (/[;&|<>`$(]/.test(command.replace(/\s\|\|\s.*$/, ""))) return ran;
  if (program === "date" && words.length === 1) {
    return { kind: "run", doing: "Checking the current date and time", done: "Checked the current date and time" };
  }
  // A plain GET of a public page sums up with the web searches, under the globe.
  if (readsTheWeb(program, words)) return { ...ran, kind: "web", web: true };
  if (program === "rg" && words.includes("--files")) return listed(null);
  if (READERS.has(program)) {
    const file = lastPath(words);
    return file ? read(file) : ran;
  }
  if (LISTERS.has(program)) return listed(lastPath(words));
  if (SEARCHERS.has(program)) {
    const [pattern = null, path = null] = words.slice(1).filter((word) => !word.startsWith("-"));
    return searchedCode(pattern, path && basename(path));
  }
  return ran;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/** A tool's name as words: "search_issues" is "search issues". */
function actionWords(short: string): string {
  return short
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .toLowerCase()
    .trim();
}

/**
 * The integration (MCP server) a tool comes from, as the user knows it: `mcp__linear__search_issues`
 * is "Linear", `mcp__claude_ai_Gmail__…` "Gmail". Plain tools have none.
 */
export function connectorName(name: string): string | null {
  const server = /^mcp__(.+?)__/.exec(name)?.[1]?.replace(/^claude_ai_/i, "");
  if (!server) return null;
  const words = server.replace(/[_-]+/g, " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "an integration";
}

/**
 * A tool's own words for `other`: what Brigadier calls it; an integration's tool by the
 * integration ("Used Linear", "Linear: search issues" while it runs, what it did and was given in
 * its fold); else "Used {name}".
 */
function otherWords(short: string, detail: string | null = null, connector: string | null = null): StepWords {
  const known = OTHER[short];
  if (known) return { kind: "other", doing: known[0], done: known[1], phrase: known[1] };
  const action = actionWords(short);
  if (connector) {
    const given = [action, detail].filter(Boolean).join(" · ");
    // While it runs, what it does too ("Pencil: batch design"): the live line may show it for long.
    const doing = action ? `${connector}: ${action}` : `Using ${connector}`;
    return { kind: "other", doing, done: `Used ${connector}`, phrase: `Used ${connector}`, unnamed: true, ...(given && { given }) };
  }
  const name = action && action !== "tool" ? action : "a tool";
  return { kind: "other", doing: `Using ${name}`, done: `Used ${name}`, phrase: `Used ${name}`, unnamed: true, ...(detail && { given: detail }) };
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
  if (CODE_SEARCH.has(short)) return searchedCode(detail);
  if (LISTINGS.has(short)) return listed(detail ? basename(detail) : null);
  if (WEB_SEARCH.has(short)) return webSearchWords(detail);
  // "Ran agent: Find the parser", "Used skill brigade" (B1).
  if (AGENTS.has(short)) {
    return detail
      ? { kind: "other", doing: `Running agent: ${detail}`, done: `Ran agent: ${detail}`, object: detail, phrase: "Ran an agent", many: "Ran agents" }
      : { kind: "other", doing: "Running an agent", done: "Ran an agent", phrase: "Ran an agent", many: "Ran agents" };
  }
  if (short === "Skill") {
    return detail
      ? { kind: "other", doing: `Using skill ${detail}`, done: `Used skill ${detail}`, object: detail, phrase: "Used a skill", many: "Used skills" }
      : { kind: "other", doing: "Using a skill", done: "Used a skill", phrase: "Used a skill", many: "Used skills" };
  }
  if (WEB_FETCH.has(short)) {
    if (!detail) return { kind: "web", doing: "Reading a web page", done: "Read a web page", web: true };
    const page = hostOf(detail);
    return { kind: "web", doing: `Reading ${page}`, done: `Read ${page}`, object: page, web: true };
  }
  return otherWords(short, detail, connectorName(call.name));
}

/**
 * A web search's words: "Searched the web for node lts | nodejs.org", its `site:` operators said
 * after a bar rather than as query words.
 */
export function webSearchWords(query: string | null): StepWords {
  const sites = [...(query ?? "").matchAll(/(?:^|\s)site:(\S+)/g)].map((match) => match[1] ?? "");
  const words = (query ?? "").replace(/(?:^|\s)site:\S+/g, " ").replace(/\s+/g, " ").trim();
  const object = [words, sites.join(", ")].filter(Boolean).join(" | ");
  if (!object) return { kind: "web", doing: "Searching the web", done: "Searched the web", web: true };
  return { kind: "web", doing: `Searching the web for ${object}`, done: `Searched the web for ${object}`, object, web: true };
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

/** A worker's file changes as the lead's `apply_patch` step names them. */
export function changesDetail(paths: readonly string[]): string | null {
  const [first] = paths;
  if (first === undefined) return null;
  return paths.length === 1 ? first : `${first} and ${paths.length - 1} more`;
}

/** The web calls whose own result (a page read, a search) takes their row, once it exists in this turn. */
const WEB_CALLS = new Set(["WebSearch", "web_search", "WebFetch", "web_fetch"]);

/**
 * What a web call's own result read or looked for, as its row's detail says it (the daemon cuts
 * that at 240 characters): its page's URL, its search's query.
 */
function webTarget(kind: { type: string }): string | null {
  const { url, query } = kind as { url?: unknown; query?: unknown };
  const target = typeof url === "string" ? url : typeof query === "string" ? query : null;
  return target === null ? null : Array.from(target).slice(0, 240).join("");
}

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
  if (kind.status !== "completed" || !WEB_CALLS.has(toolName(kind.name))) return false;
  // A web call's result names its page or query: it is that call's, however many calls ran
  // alongside it (several pages read at once each end apart, after the others started).
  // A Codex search's call can open a page: its result is then a page read.
  return !!kind.detail && !!results?.some(
    (step) =>
      step.requestId === requestId &&
      (step.kind.type === "readPage" || step.kind.type === "searchedWeb") &&
      step.position >= position &&
      webTarget(step.kind) === kind.detail,
  );
}

/**
 * A step's row: its words for how it stands. A command reads as the call it was however it
 * ended ("Ran pnpm test"): its exit code is in its Shell box, and a declined call is counted in
 * the turn's header. Another tool that failed says so ("Used a tool — failed").
 */
export function stepLabel(words: StepWords, status: ItemStatus, _exit: number | null = null): string {
  if (status === "inProgress") return words.doing;
  const command = words.kind === "run" || (words.kind === "web" && words.done.startsWith("Ran "));
  if (status === "failed" && !command) return `${words.done} — failed`;
  return words.done;
}

/** The shell tools of CLIs whose records keep no run time of a command: Claude Code's. */
const UNTIMED_SHELLS = new Set(["Bash"]);

/** How far apart a call's start or end and its approval's answer can be on the daemon's clock. */
const ANSWER_SLACK_MS = 1000;

/**
 * When a lead's command the user had to allow began to run: the thread's own approval (not a
 * worker's) of the same command, allowed while the call waited (from `atMs`, when it was first
 * seen, to its end). A CLI whose record keeps no run time of its own times the call from when it
 * asked, so its run time would count the wait for the user's answer; none when no approval held
 * it up, or the CLI timed the command itself.
 */
export function allowedAt(call: ToolStep, atMs: number | undefined, approvals: readonly Approval[]): number | undefined {
  if (call.item !== "command" || call.detail === null || atMs === undefined) return undefined;
  let at: number | undefined;
  for (const approval of approvals) {
    if (approval.taskId !== null || approval.state.type !== "allowed" || approval.subject.type !== "cli") continue;
    const { request } = approval.subject;
    if (!UNTIMED_SHELLS.has(request.tool) || request.command === null || approval.resolvedAtMs === null) continue;
    if ([...unwrapCommand(request.command)].slice(0, 240).join("") !== call.detail) continue;
    if (approval.resolvedAtMs < atMs - ANSWER_SLACK_MS) continue;
    if (call.endedAtMs !== undefined && approval.resolvedAtMs > call.endedAtMs + ANSWER_SLACK_MS) continue;
    at = Math.max(at ?? atMs, Math.min(approval.resolvedAtMs, call.endedAtMs ?? approval.resolvedAtMs));
  }
  return at;
}

/**
 * Whether a lead's failed call is one an approval denied: the thread's own approval (not a
 * worker's) in one of the turn's requests, for the same tool, answered while the call ran (from
 * `atMs`, when it was first seen). A CLI reports a denied call as a failed one; its row reads as
 * declined instead, never as an error.
 */
export function deniedCall(
  call: ToolStep,
  atMs: number | undefined,
  requestIds: readonly string[],
  approvals: readonly Approval[],
): boolean {
  if (call.status !== "failed") return false;
  return approvals.some(
    (approval) =>
      approval.taskId === null &&
      approval.requestId !== null &&
      requestIds.includes(approval.requestId) &&
      approval.state.type === "denied" &&
      approval.subject.type === "cli" &&
      toolName(approval.subject.request.tool) === toolName(call.name) &&
      approval.resolvedAtMs !== null &&
      (atMs === undefined || approval.resolvedAtMs >= atMs - ANSWER_SLACK_MS) &&
      (call.endedAtMs === undefined || approval.resolvedAtMs <= call.endedAtMs + ANSWER_SLACK_MS),
  );
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
  const words = stepWords({ ...kind, detail: worker ? workerName(tasks, worker) : kind.detail }, tasks);
  // "Running sleep 20 in the background" once its turn ended (D44).
  return kind.background && kind.status === "inProgress" ? { ...words, doing: `${words.doing} in the background` } : words;
}

/** How a kind leads a summary, for one step and for several. Reading, searching and listing all read as "Read files". */
const LEADING: Record<Exclude<WorkKind, "other" | "code">, [one: string, many: string]> = {
  read: ["Read files", "Read files"],
  edit: ["Edited a file", "Edited files"],
  run: ["Ran a command", "Ran commands"],
  web: ["Searched the web", "Searched the web"],
  message: ["Messaged a session", "Messaged sessions"],
};

/** "A, B and C". */
export function joinWords(parts: readonly string[]): string {
  if (parts.length <= 1) return parts[0] ?? "";
  return `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`;
}

/** The kinds a summary names first, in this order; the rest follow in the order they first came. */
const SUMMARY_ORDER: readonly string[] = ["edit", "read", "run", "web"];

function summaryRank(key: string): number {
  const at = SUMMARY_ORDER.indexOf(key);
  return at < 0 ? SUMMARY_ORDER.length : at;
}

/**
 * A run of steps in one sentence, comma-joined: edits, then what it read, searched or listed,
 * then commands and the web, then the rest in the order they first came ("Edited a file, read
 * files, ran commands"). Each `other` tool counts as a kind of its own; one Brigadier has no
 * words for says how often it was used ("used Linear 6 times").
 */
export function summarize(steps: readonly StepWords[]): string {
  const parts = segments(steps).map(([key, { words, count }]) =>
      words.kind === "other"
        ? count > 1 && words.many
          ? words.many
          : `${words.phrase ?? words.done}${words.unnamed && count > 1 ? ` ${timesWord(count)}` : ""}`
        : LEADING[key as keyof typeof LEADING][count === 1 ? 0 : 1],
    );
  return parts.map((part, index) => (index === 0 ? part : part.charAt(0).toLowerCase() + part.slice(1))).join(", ");
}

/**
 * A run of steps by summary segment, in the summary's order: each segment's first step and how
 * many. Edits that name their files count the distinct files: two edits of one file are "Edited
 * a file", one patch of two files "Edited files".
 */
function segments(steps: readonly StepWords[]): [string, { words: StepWords; count: number }][] {
  const counts = new Map<string, { words: StepWords; count: number }>();
  const paths = new Map<string, Set<string>>();
  for (const words of steps) {
    const key = words.kind === "other" ? `other:${words.phrase ?? words.done}` : words.kind === "code" ? "read" : words.kind;
    const known = counts.get(key);
    let count = (known?.count ?? 0) + 1;
    if (words.paths?.length) {
      const seen = paths.get(key) ?? new Set<string>();
      const before = seen.size;
      for (const path of words.paths) seen.add(path);
      paths.set(key, seen);
      count = (known?.count ?? 0) + (seen.size - before);
    }
    counts.set(key, { words: known?.words ?? words, count });
  }
  return [...counts.entries()].toSorted(([a], [b]) => summaryRank(a) - summaryRank(b));
}

/** "2 times", "6 times". */
export function timesWord(count: number): string {
  return `${count} times`;
}

/** Back-to-back calls of one tool Brigadier has no words for fold into one row from this many on (#101). */
export const FOLD_REPEATS_AT = 3;

/**
 * A group's steps as its opened list shows them (#101): each step on its own (`[index]`), except
 * `FOLD_REPEATS_AT` or more back-to-back calls of one tool Brigadier has no words for, which
 * make one run (`[first, …, last]`).
 */
export function foldRepeats(steps: readonly StepWords[]): number[][] {
  const runs: number[][] = [];
  steps.forEach((words, index) => {
    const run = runs.at(-1);
    const previous = run === undefined ? undefined : steps[run[0] ?? -1];
    if (run && words.unnamed && previous?.unnamed && previous.done === words.done) run.push(index);
    else runs.push([index]);
  });
  return runs.flatMap((run) => (run.length >= FOLD_REPEATS_AT ? [run] : run.map((index) => [index])));
}

/** The step whose icon a summary row shows: the first of the summary's first segment ("Ran commands, …" → a terminal). */
export function summaryLead(steps: readonly StepWords[]): StepWords | undefined {
  return segments(steps)[0]?.[1].words;
}

/** A worker's action, as its transcript keeps it. */
export type ActionItem = Extract<TranscriptItem, { kind: "command" | "tool" | "files" | "image" }>;

/** A worker's action as the call the lead's step would record for it. */
export function itemCall(item: ActionItem): StepCall {
  switch (item.kind) {
    case "command":
      return { name: "shell", detail: item.command, status: item.status };
    case "tool":
      return { name: item.name, detail: agentDetail(item.name, item.input) ?? detailOf(item.input), status: item.status };
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
    : { name: call.name, detail: agentDetail(call.name, call.input) ?? detailOf(call.input), status: "inProgress" }).doing;
}
