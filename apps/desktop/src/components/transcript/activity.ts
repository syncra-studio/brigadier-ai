import { toolActivity } from "@/app/conversation/toolWords";
import type { TranscriptItem } from "@/components/transcript/transcript";

/**
 * A worker's tool use, told as: one grey line per action ("Read notes.py", "Ran
 * git status"), and each run of actions between two replies summed up in one line ("Read
 * files, ran commands").
 */

export type ActivityKind = "read" | "list" | "search" | "edit" | "run" | "report" | "tool";

export type Activity = {
  kind: ActivityKind;
  /** It reaches the network (curl, git fetch, …): shown as a globe. */
  web?: boolean;
  /** "Read notes.py", "Ran git status". */
  done: string;
  /** While it runs: "Reading notes.py". */
  doing: string;
};

type ActionItem = Extract<TranscriptItem, { kind: "command" | "tool" | "files" | "image" }>;

/** One entry of a worker's thread: a reply or row as it is, or a run of actions. */
export type ThreadEntry =
  | { kind: "item"; item: TranscriptItem }
  | { kind: "actions"; key: string; items: ActionItem[] };

const READERS = new Set(["cat", "head", "tail", "nl", "less", "more", "bat", "wc", "sed"]);
const LISTERS = new Set(["ls", "find", "tree", "fd"]);
const SEARCHERS = new Set(["rg", "grep", "ag", "ack"]);
const NETWORK = new Set(["curl", "wget", "http", "ping", "ssh", "scp", "nc", "dig"]);
const NETWORK_GIT = new Set(["fetch", "pull", "push", "clone", "ls-remote"]);

/** The command a shell wrapper runs: `/bin/zsh -c 'wc -l notes.py'` → `wc -l notes.py`. */
export function unwrapCommand(command: string): string {
  const wrapped = /^(?:\/\S*\/)?(?:ba|z)?sh\s+-l?c\s+(['"])([\s\S]*)\1$/.exec(command.trim());
  return (wrapped?.[2] ?? command).trim();
}

/** Brigadier's own folder of tool shims, before a tool's name: `/…/gate/bin/git` → `git`. */
const GATE_PATH = /(^|[\s'"(;&|])(?:\/[\w.@+-]+(?: [\w.@+-]+)*)*?\/gate\/bin\//g;

/** git's `-c name=value` settings right after `git`. */
const GIT_SETTINGS = /(\bgit)((?:\s+-c\s+[\w.-]+=\S*)+)/g;

/**
 * Settings that change only how git prints or keeps its index, never what a command does.
 * Any other setting (`core.hooksPath`, `user.email`, an alias) stays in view: it can change
 * what the user is asked to allow.
 */
const QUIET_SETTING = /^(?:core\.splitIndex|merge\.autoStash|core\.quotePath|core\.pager|color\.[\w.-]+|advice\.[\w.-]+|pager\.[\w.-]+|column\.ui|gc\.auto|maintenance\.auto)=/i;

function withoutQuietSettings(git: string, settings: string): string {
  const kept = [...settings.matchAll(/\s+-c\s+(\S+)/g)]
    .filter((match) => !QUIET_SETTING.test(match[1] ?? ""))
    .map((match) => match[0]);
  return git + kept.join("");
}

/**
 * A command as the card shows it: what the shell wrapper runs, with Brigadier's gate folder
 * and git's display-only `-c` settings left out (`/bin/zsh -lc '/…/gate/bin/git -c
 * color.ui=never push'` → `git push`).
 */
export function shownCommand(command: string): string {
  return unwrapCommand(command).replace(GATE_PATH, "$1").replace(GIT_SETTINGS, (_, git: string, settings: string) => withoutQuietSettings(git, settings)).trim();
}

function basename(path: string): string {
  return path.replace(/\/+$/, "").split("/").pop() || path;
}

/** A simple command's words, quotes removed: `grep -n "def test_" tests` → 4 words. */
function wordsOf(command: string): string[] {
  return [...command.matchAll(/'([^']*)'|"([^"]*)"|(\S+)/g)].map(
    (match) => match[1] ?? match[2] ?? match[3] ?? "",
  );
}

/** The last path-like argument, if any (`sed -n 1,80p notes.py` → notes.py). */
function target(words: readonly string[]): string | null {
  const last = words
    .slice(1)
    .filter((word) => !word.startsWith("-") && !/^\d/.test(word))
    .at(-1);
  return last ? basename(last) : null;
}

function classifyCommand(raw: string): Activity {
  const command = unwrapCommand(raw);
  // A compound command is told as what it runs.
  const simple = !/[;&|<>`$(]/.test(command.replace(/\s\|\|\s.*$/, ""));
  const words = wordsOf(command);
  const program = basename(words[0] ?? "");
  const firstLine = command.split("\n")[0] ?? command;
  const web =
    NETWORK.has(program) || (program === "git" && NETWORK_GIT.has(words[1] ?? "")) || undefined;
  const ran: Activity = {
    kind: "run",
    done: `Ran ${firstLine}`,
    doing: `Running ${firstLine}`,
    ...(web && { web }),
  };
  if (!simple) return ran;
  if (program === "rg" && words.includes("--files")) {
    return { kind: "list", done: "Listed files", doing: "Listing files" };
  }
  if (READERS.has(program)) {
    const file = target(words);
    return file ? { kind: "read", done: `Read ${file}`, doing: `Reading ${file}` } : ran;
  }
  if (LISTERS.has(program)) {
    const dir = target(words);
    return dir
      ? { kind: "list", done: `Listed files in ${dir} folder`, doing: `Listing files in ${dir} folder` }
      : { kind: "list", done: "Listed files", doing: "Listing files" };
  }
  if (SEARCHERS.has(program)) {
    const pattern = words.slice(1).find((word) => !word.startsWith("-"));
    const what = pattern ? ` for ${pattern}` : "";
    return { kind: "search", done: `Searched${what}`, doing: `Searching${what}` };
  }
  return ran;
}

function parseInput(input: string | null): Record<string, unknown> {
  if (!input) return {};
  try {
    const value: unknown = JSON.parse(input);
    return value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function classifyTool(name: string, input: string | null): Activity {
  const args = parseInput(input);
  const text = (key: string) => (typeof args[key] === "string" ? (args[key] as string) : null);
  const path = text("file_path") ?? text("path") ?? text("notebook_path");
  const file = path ? basename(path) : null;
  const short = name.replace(/^mcp__/, "").replace(/__/g, "/");
  switch (name) {
    case "Read":
      return { kind: "read", done: `Read ${file ?? "a file"}`, doing: `Reading ${file ?? "a file"}` };
    case "Glob":
    case "LS":
      return { kind: "list", done: "Listed files", doing: "Listing files" };
    case "Grep": {
      const pattern = text("pattern");
      const what = pattern ? ` for ${pattern}` : "";
      return { kind: "search", done: `Searched${what}`, doing: `Searching${what}` };
    }
    case "WebSearch": {
      const query = text("query");
      const what = query ? ` for ${query}` : "";
      return { kind: "search", web: true, done: `Searched the web${what}`, doing: `Searching the web${what}` };
    }
    case "WebFetch": {
      const url = text("url");
      let host = "a page";
      try {
        if (url) host = new URL(url).host || url;
      } catch {
        host = url ?? host;
      }
      return { kind: "read", web: true, done: `Read ${host}`, doing: `Reading ${host}` };
    }
    case "Edit":
    case "MultiEdit":
    case "Write":
    case "NotebookEdit":
      return { kind: "edit", done: `Edited ${file ?? "a file"}`, doing: `Editing ${file ?? "a file"}` };
  }
  if (short.endsWith("submit_report")) {
    return { kind: "report", done: "Sent message to parent", doing: "Sending message to parent" };
  }
  const activity = toolActivity(name);
  const kind = activity.kind === "web" ? "search" : ["read", "list", "search", "edit", "run", "report"].includes(activity.kind) ? activity.kind as ActivityKind : "tool";
  return { kind, done: activity.done, doing: activity.doing, ...(activity.kind === "web" && { web: true }) };
}

/** What a worker's live command or tool call is doing, in the thread's words ("Reading notes.py"). */
export function liveActivity(call: { command: string } | { name: string; input: string | null }): string {
  return ("command" in call ? classifyCommand(call.command) : classifyTool(call.name, call.input)).doing;
}

export function activityOf(item: ActionItem): Activity {
  switch (item.kind) {
    case "command":
      return classifyCommand(item.command);
    case "tool":
      return classifyTool(item.name, item.input);
    case "files": {
      const names = item.changes.map((change) => basename(change.path));
      const label = names.length === 1 ? names[0] : `${names.length} files`;
      return { kind: "edit", done: `Edited ${label}`, doing: `Editing ${label}` };
    }
    case "image":
      return { kind: "tool", done: "Made an image", doing: "Making an image" };
  }
}

/** Tool calls the worker's CLI makes for itself, with nothing to tell. */
function hidden(item: TranscriptItem): boolean {
  return item.kind === "tool" && item.name === "ToolSearch";
}

function isAction(item: TranscriptItem): item is ActionItem {
  return item.kind === "command" || item.kind === "tool" || item.kind === "files" || item.kind === "image";
}

/**
 * A worker's transcript as its thread shows it: replies and reasoning stay, adjacent actions become one run, turn markers go. A failed or stopped turn stays.
 */
export function threadEntries(items: readonly TranscriptItem[]): ThreadEntry[] {
  const entries: ThreadEntry[] = [];
  for (const item of items) {
    if (hidden(item) || item.kind === "turnStarted") continue;
    if (item.kind === "turnCompleted" && item.status === "completed") continue;
    if (isAction(item)) {
      const last = entries.at(-1);
      if (last?.kind === "actions" && activityOf(item).kind !== "report" && last.items.every((action) => activityOf(action).kind !== "report")) last.items.push(item);
      else entries.push({ kind: "actions", key: item.key, items: [item] });
      continue;
    }
    entries.push({ kind: "item", item });
  }
  return entries;
}

const PLURALS: Record<ActivityKind, [one: string, many: string]> = {
  read: ["read a file", "read files"],
  list: ["listed files", "listed files"],
  search: ["searched", "searched"],
  edit: ["edited a file", "edited files"],
  run: ["ran a command", "ran commands"],
  report: ["sent message to parent", "sent messages to parent"],
  tool: ["used a tool", "used tools"],
};

/** "Read files, ran a command": the kinds of a run, in the order they first happened. */
export function summarize(activities: readonly Activity[]): string {
  const counts = new Map<ActivityKind, number>();
  for (const activity of activities) counts.set(activity.kind, (counts.get(activity.kind) ?? 0) + 1);
  const parts = [...counts].map(([kind, count]) => PLURALS[kind][count === 1 ? 0 : 1]);
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}
