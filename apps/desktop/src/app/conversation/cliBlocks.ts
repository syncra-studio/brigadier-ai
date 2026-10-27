import { agentDetail, brigadierCalls, brigadierUnshown, isPlumbing, isSubagentStart, toolName, type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import {
  type Block,
  type BlockCompaction,
  type BoardDigest,
  type BlockState,
  blockShows,
  boardItems,
  isLive,
  isUpdate,
  type ThreadNode,
  type ThreadTree,
} from "@/app/conversation/blocks";
import { sessionSent } from "@/app/conversation/sessionSend";
import { shownText } from "@/app/conversation/streamRelease";
import { INTERRUPTED } from "@/app/conversation/turnError";
import type {
  BackgroundTask,
  FileChange,
  FileChangeKind,
  FileEntry,
  FileStat,
  Message,
  ThreadItemKind,
  RunState,
  ThreadTurn,
  ThreadView,
} from "@/ipc/generated";
import { sessionImageRef } from "@/state/cliThread";
import { shownIdOf } from "@/state/shownIds";
import type { PendingMessage } from "@/state/store";

/**
 * Internal continuation markers, also recognized with an older daemon or incomplete seam
 * metadata. Whose a message is comes from the engine's note of it (`FileEntry.said`, L12); these
 * words decide only for one it has no note of, and say what one of Brigadier's is.
 */
const NUDGE_MARK = "[Brigadier] A fresh session will continue this work.";
const HANDOFF_MARK = "[Brigadier hand-off ";
const HANDOFF_END = "[End of the hand-off]";
const CARRY_ON = "Carry on from the hand-off note.";

/**
 * Automatic engine messages to the CLI are no bubble: a card's answer, a resume, a background
 * command's result, a preview's end, a carry-on after a limit, or the workers the user stopped.
 * Peer messages retain their sender and authored content. The CLI's resulting work still shows.
 * Prefix matching is only a fallback for entries without the engine's provenance note.
 */
const BRIGADIER_SAYS =
  /^\[(?:decision\] |answer\] |not finished\] |merge\] |merge cards retired\]|preview ended: |your background command |The user stopped you, then asked you to resume\.|Brigadier quit |Brigadier restarted |Brigadier: this conversation continues |Your last turn stopped early|worker\] The user stopped )/;

/** How Brigadier's word that a Codex command ended after its turn starts (`conversation.rs`). */
const CODEX_WAKE = /^\[your background command /;

/** A notice the CLI starts a turn of its own with, not the user's: a background job's end. */
const CLI_NOTICE = /^(?:<task-notification>|\[your background command )/;

/** Whether a call is the hand-off's upkeep the nudge asks for: the ledger, or the note's file. */
function upkeep(event: FileEntry["event"]): boolean {
  const said = event.type === "toolCall" ? (event.input ?? "") : event.type === "command" ? event.command : event.type === "fileChanges" ? event.changes.map((change) => change.path).join("\n") : null;
  return said !== null && (said.includes("brigadier ledger") || /[/\\]record[/\\]handoff[/\\]/.test(said));
}

/** How Brigadier's checkpoint request starts (`long_sessions/checkpoint.rs` ASK_MARK). */
const CHECKPOINT_MARK = "[Brigadier · checkpoint]";

/**
 * The thread, drawn from the CLI's own session file (S9, S10): the engine's `ThreadView` as
 * blocks, one per turn. A turn is the user's message up to the CLI's end of it; its replies,
 * reasoning and calls are the CLI's items in the CLI's order, and its time runs by the CLI's
 * timestamps. What Brigadier keeps of its own (cards, workers, their rows) shows in the turn of
 * the request it belongs to, among the CLI's items by time.
 */

/** A tool's own name, without its MCP server's prefix (`mcp__linear__search_issues` → `search_issues`). */
const bare = toolName;

/** Shell tools: their call is a command (`digest::is_shell_tool`). */
const SHELL_TOOLS = new Set(["Bash"]);

const IMAGE_FILE = /\.(png|jpe?g|gif|webp|bmp|tiff|heic)$/i;

function parsed(input: string | null): Record<string, unknown> | null {
  if (!input) return null;
  try {
    const value: unknown = JSON.parse(input);
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** What a tool call is as a transcript item, as the engine says it (`thread_items::tool_item`). */
export function toolItem(name: string, input: string | null): ThreadItemKind {
  const short = bare(name);
  if (SHELL_TOOLS.has(short)) return "command";
  switch (short) {
    case "WebSearch":
    case "WebFetch":
    case "web_search":
      return "web";
    case "view_image":
      return "image";
    case "Read": {
      const path = parsed(input)?.["file_path"];
      return typeof path === "string" && IMAGE_FILE.test(path) ? "image" : "tool";
    }
    case "Edit":
    case "MultiEdit":
    case "Write":
    case "NotebookEdit":
    case "apply_patch":
      return "files";
    default:
      return "tool";
  }
}

/**
 * A call's one-line detail on its row, from its arguments (`thread_items::step_detail`): a find
 * in a page by what it looked for and where, else the first of its query, pattern, path, URL,
 * title, task, name or command.
 */
export function stepDetail(input: string | null): string | null {
  const args = parsed(input);
  if (!args) return null;
  const text = (key: string) => (typeof args[key] === "string" ? (args[key] as string) : undefined);
  const pattern = text("pattern");
  const url = text("url");
  const found =
    text("action") === "findInPage" && pattern !== undefined && url !== undefined
      ? `'${pattern}' in ${url}`
      : ["query", "pattern", "file_path", "path", "url", "title", "task", "name", "command"].map(text).find((value) => value !== undefined);
  return found === undefined ? null : [...found].slice(0, 240).join("");
}

/**
 * A result's text: a JSON content array's text blocks (a Codex MCP call's), one per line, else
 * the result as it is (`digest::content_text`).
 */
function contentText(output: string): string {
  const trimmed = output.trimStart();
  if (!trimmed.startsWith("[") && !trimmed.startsWith("{")) return output;
  try {
    const value: unknown = JSON.parse(output);
    const blocks = Array.isArray(value) ? value : (value as { content?: unknown } | null)?.content;
    if (!Array.isArray(blocks) || blocks.length === 0) return output;
    const texts = blocks.map((block: { type?: unknown; text?: unknown }) => (block?.type === "text" && typeof block.text === "string" ? block.text : null));
    return texts.every((text) => text !== null) ? texts.join("\n") : output;
  } catch {
    return output;
  }
}

/** The exit code a shell tool's result names on its first line (`digest::exit_of`). */
export function exitOf(output: string): number | undefined {
  const first = contentText(output).split("\n")[0]?.trimStart() ?? "";
  const found = /^(?:\[exit |exit |Exit code )(-?\d+)/.exec(first);
  return found ? Number(found[1]) : undefined;
}

/** Lines in and out of a diff (`model::diff_counts`). */
function diffCounts(kind: FileChangeKind, diff: string): [number, number] {
  let lines = diff.split("\n");
  if (lines.at(-1) === "") lines = lines.slice(0, -1);
  if (lines.length >= 2 && lines[0]?.startsWith("--- ") && lines[1]?.startsWith("+++ ")) lines = lines.slice(2);
  lines = lines.filter((line) => !line.startsWith("@@"));
  if (kind === "add") return [lines.length, 0];
  if (kind === "delete") return [0, lines.length];
  return [lines.filter((line) => line.startsWith("+")).length, lines.filter((line) => line.startsWith("-")).length];
}

/** A file change's files with their lines in and out (`thread_items::file_stats`). */
export function fileStats(changes: readonly FileChange[]): FileStat[] {
  return changes.map((change) => {
    const [insertions, deletions] =
      change.added !== undefined && change.removed !== undefined
        ? [change.added, change.removed]
        : change.diff !== undefined
          ? diffCounts(change.kind, change.diff)
          : [0, 0];
    return {
      path: change.movedTo ?? change.path,
      insertions,
      deletions,
      binary: false,
      change: change.kind,
      ...(change.movedTo !== undefined && { from: change.path }),
    };
  });
}

/** When a call ended, by the CLI's timestamps: none while it runs. */
function endedAt(entry: FileEntry, status: ToolStep["status"], durationMs?: number | null): number | undefined {
  if (status === "inProgress") return undefined;
  if (durationMs != null && entry.atMs !== undefined) return entry.atMs + durationMs;
  return entry.updatedAtMs ?? entry.atMs;
}

/**
 * A call as its row, in the words the engine gives a call of the live stream
 * (`conversation.rs`'s tool, command and file change rows, `thread_images::image_row`); `null`
 * for what is no row: the CLIs' plumbing, whose trace is elsewhere.
 */
export function toolStep(entry: FileEntry, stopped: boolean): ToolStep | null {
  const { event } = entry;
  const base = { throughPosition: 0 };
  switch (event.type) {
    case "toolCall": {
      const name = bare(event.name);
      if (isPlumbing(event.name)) return null;
      const endedAtMs = endedAt(entry, event.status);
      const exit = SHELL_TOOLS.has(name) && endedAtMs !== undefined && event.output !== null ? exitOf(event.output) : undefined;
      return {
        type: "tool",
        itemId: event.itemId,
        // As the CLI named it: an MCP call's server is part of its words ("Used Linear").
        name: event.name,
        detail: agentDetail(event.name, event.input) ?? stepDetail(event.input),
        status: event.status,
        ...(stopped && event.status === "failed" && { stopped: true }),
        ...base,
        ...(endedAtMs !== undefined && { endedAtMs }),
        ...(exit !== undefined && { exit }),
        item: toolItem(name, event.input),
      };
    }
    case "command": {
      const endedAtMs = endedAt(entry, event.status, event.durationMs);
      return {
        type: "tool",
        itemId: event.itemId,
        name: "shell",
        detail: [...unwrapCommand(event.command)].slice(0, 240).join(""),
        status: event.status,
        ...(stopped && event.status === "failed" && { stopped: true }),
        ...base,
        ...(endedAtMs !== undefined && { endedAtMs }),
        ...(event.exitCode !== null && { exit: event.exitCode }),
        item: "command",
      };
    }
    case "fileChanges": {
      const [first] = event.changes;
      const endedAtMs = endedAt(entry, event.status);
      return {
        type: "tool",
        itemId: event.itemId,
        name: "apply_patch",
        detail: first ? (event.changes.length === 1 ? first.path : `${first.path} and ${event.changes.length - 1} more`) : null,
        status: event.status,
        ...base,
        ...(endedAtMs !== undefined && { endedAtMs }),
        item: "files",
        files: fileStats(event.changes),
      };
    }
    case "image": {
      const endedAtMs = endedAt(entry, event.status);
      return {
        type: "tool",
        itemId: event.itemId,
        name: "imagegen",
        detail: event.prompt,
        status: event.status,
        ...base,
        ...(endedAtMs !== undefined && { endedAtMs }),
        item: "image",
      };
    }
    default:
      return null;
  }
}

/**
 * A call's row as its turn and the CLI's background tasks leave it (D44). A call the CLI runs in
 * the background stands as its task does: running while the task runs, "in the background" once
 * its turn ended (the call itself answered at once), then as the task ended. Any other call
 * still running when its turn ended was cut off with it: it reads stopped, never running for good.
 */
/** Claude's own plan file, which its plan mode writes the plan to. */
const PLAN_FILE = /(?:^|\/)\.claude\/plans\/[^/]+\.md$/;

/**
 * A CLI's subagent completion notice belongs to its strip and History, like its start call.
 */
function subagentNotice(text: string): boolean {
  return /^Agent "[\s\S]*" (?:finished|completed|failed|was stopped|stopped|was killed|killed)\b/.test(text.trim());
}

export function stepInTurn(step: ToolStep | null, turn: Pick<ThreadTurn, "end" | "endedAtMs"> | null, task: BackgroundTask | undefined): ToolStep | null {
  if (!step) return null;
  // The thread's own `brigadier wait` and `start` are no rows (D47): the block's "Waiting for a
  // worker" line and the worker's own row say them.
  if (step.item === "command" && step.detail !== null && brigadierUnshown(step.detail)) return null;
  // Claude's plan, written to its own plan file in plan mode, is the plan card's, as Codex's
  // plan is: no "Edited …" row (harness D7, D57).
  if (step.item === "files" && (step.files?.length ?? 0) > 0 && step.files?.every((file) => PLAN_FILE.test(file.path))) return null;
  const ended = turn?.end != null;
  const { endedAtMs: _endedAtMs, exit: _exit, stopped: _stopped, ...running } = step;
  if (task?.status === "running") return { ...running, status: "inProgress", ...(ended && { background: true }) };
  const taskEnd = task?.endedAtMs ?? undefined;
  if (task?.status === "stopped") return { ...running, status: "failed", stopped: true, ...(taskEnd !== undefined && { endedAtMs: taskEnd }) };
  if (task?.status === "failed") return { ...step, status: "failed", ...(taskEnd !== undefined && { endedAtMs: taskEnd }) };
  // Its task completed: so did the call, even when the CLI never said so of the item itself (a
  // Codex terminal that left the list first).
  if (task && taskEnd !== undefined) return { ...step, ...(step.status === "inProgress" && { status: "completed" as const }), endedAtMs: Math.max(taskEnd, step.endedAtMs ?? taskEnd) };
  if (step.status !== "inProgress" || !ended) return step;
  return { ...running, status: "failed", stopped: true, ...(turn?.endedAtMs != null && { endedAtMs: turn.endedAtMs }) };
}

/** How far a "Messaged …" row, or a card's row, can be from the run of the `brigadier` call that made it. */
const SEND_SLACK_MS = 2000;

/**
 * A `brigadier send` whose message has its own "Messaged …" row (a worker's, a session's), and a
 * `brigadier ask` whose card has its own "Asking questions" row, is no row of its own: the two
 * said the same (D47, D58).
 */
function dropShownCalls(blocks: readonly Block[], board: BoardDigest): void {
  for (const block of blocks) {
    const sent = block.orchestratorSteps.flatMap((step) => ((step.kind.type === "messaged" || step.kind.type === "peerSent") && step.atMs !== undefined ? [step.atMs] : []));
    const asked = block.cards.flatMap((card) => {
      const question = card.type === "question" ? board.questions[card.id] : undefined;
      return question?.kind.type === "orchestrator" ? [question.createdAtMs] : [];
    });
    if (sent.length === 0 && asked.length === 0) continue;
    block.orchestratorSteps = block.orchestratorSteps.filter((step) => {
      const { kind, atMs } = step;
      if (kind.type !== "tool" || kind.item !== "command" || kind.detail === null || atMs === undefined) return true;
      const sub = brigadierCalls(kind.detail)[0];
      const shown = sub === "send" ? sent : sub === "ask" ? asked : [];
      const [from, to] = [atMs - SEND_SLACK_MS, (kind.endedAtMs ?? atMs) + SEND_SLACK_MS];
      return !shown.some((at) => at >= from && at <= to);
    });
  }
}

/**
 * Whether a background job's end is that of the thread's own `brigadier wait` (Claude's notice
 * names the task by its description, Brigadier's Codex word by its command): no line, as the
 * wait had no row (D38, D47); the worker rows after it say what happened.
 */
function waitEnded(text: string, view: ThreadView): boolean {
  const codex = /^\[your background command `([\s\S]*?)` finished after your turn/.exec(text);
  if (codex) return brigadierCalls(codex[1] ?? "")[0] === "wait";
  const name = /^Background command "([\s\S]*)" (?:completed|failed|was stopped|was killed|stopped|killed)\b/.exec(text.trim())?.[1];
  if (name === undefined) return false;
  if (brigadierCalls(name)[0] === "wait") return true;
  const task = view.background?.find((each) => each.label === name && each.itemId !== null);
  const call = task && view.entries.find((entry) => "itemId" in entry.event && entry.event.itemId === task.itemId)?.event;
  const command = call?.type === "command" ? call.command : call?.type === "toolCall" ? stepDetail(call.input) : null;
  return command != null && brigadierCalls(command)[0] === "wait";
}

/** Where Claude writes a background command's output, as its call's result says. */
const OUTPUT_FILE = /Output is being written to: (\S+?\.output)\b/;

/** A path as one file whichever name macOS gives `/tmp` (`/private/tmp`). */
const samePath = (path: string) => path.replace(/^\/private\//, "/");

/**
 * The output files of the thread's own `brigadier wait`s, from the wait's own result (a reload
 * keeps it) or its background task: reading one is part of the wait, under the hood too (D38, D58).
 */
function waitOutputs(view: ThreadView): Set<string> {
  const files = new Set<string>();
  for (const { event } of view.entries) {
    if (event.type !== "command" && event.type !== "toolCall") continue;
    const command = event.type === "command" ? event.command : SHELL_TOOLS.has(bare(event.name)) ? stepDetail(event.input) : null;
    if (command == null || brigadierCalls(command)[0] !== "wait") continue;
    const said = event.output && OUTPUT_FILE.exec(event.output)?.[1];
    if (said) files.add(samePath(said));
    const task = view.background?.find((each) => each.itemId === event.itemId);
    if (task?.outputFile) files.add(samePath(task.outputFile));
  }
  return files;
}

/** Whether a call only reads one of `files`: a Read of it, or a plain `cat`, `head` or `tail` of it. */
function readsOnly(event: FileEntry["event"], files: ReadonlySet<string>): boolean {
  if (files.size === 0) return false;
  if (event.type === "toolCall" && bare(event.name) === "Read") {
    const path = parsed(event.input)?.["file_path"];
    return typeof path === "string" && files.has(samePath(path));
  }
  const command = event.type === "command" ? event.command : event.type === "toolCall" && SHELL_TOOLS.has(bare(event.name)) ? stepDetail(event.input) : null;
  const path = command && /^(?:cat|head|tail)(?:\s+-[\w-]+(?:\s+\d+)?)*\s+(?:'([^']+)'|"([^"]+)"|(\S+))$/.exec(unwrapCommand(command));
  return !!path && files.has(samePath(path[1] ?? path[2] ?? path[3] ?? ""));
}

/** Each entry's row as last made: an item the stream left alone keeps its entry (`mergeEntries`). */
const madeSteps = new WeakMap<FileEntry, { made: string; step: ToolStep | null }>();

/** `toolStep`, made once per entry and what it depends on: the thread is made again with each streamed word. */
function keptToolStep(entry: FileEntry, turn: ThreadTurn | null, background: readonly BackgroundTask[] | undefined): ToolStep | null {
  if (entry.event.type === "toolCall" && isSubagentStart(entry.event.name)) return null;
  const itemId = "itemId" in entry.event ? entry.event.itemId : undefined;
  const task = itemId === undefined ? undefined : background?.find((each) => each.itemId === itemId);
  const made = `${turn?.end ?? ""}|${turn?.endedAtMs ?? ""}|${task?.status ?? ""}|${task?.endedAtMs ?? ""}`;
  const kept = madeSteps.get(entry);
  if (kept?.made === made) return kept.step;
  const step = stepInTurn(toolStep(entry, turn?.end === "stopped"), turn, task);
  madeSteps.set(entry, { made, step });
  return step;
}

/** A turn's state in the words the thread's blocks use, with its error. */
export function turnState(turn: Pick<ThreadTurn, "end" | "error">): { state: BlockState; error: string | null } {
  switch (turn.end) {
    case null:
      return { state: "working", error: null };
    case "done":
      return { state: "done", error: null };
    case "stopped":
      return { state: "stopped", error: null };
    case "failed":
      return { state: "failed", error: turn.error };
    // The CLI went mid-turn (killed, or Brigadier quit under it): it reads interrupted, with
    // Continue, as a turn Brigadier quit under always has.
    case "interrupted":
      return { state: "failed", error: turn.error ?? INTERRUPTED };
  }
}

/** The user's message of a turn (or a steer) as the thread's message record: Brigadier's, by its request, when the turn names it. */
function userMessage(conversationId: string, entry: FileEntry, id: string, text: string, seq: number): Message {
  return {
    id,
    conversationId,
    seq,
    role: "user",
    text,
    blob: null,
    createdAtMs: entry.atMs ?? 0,
    attachments: (entry.images ?? []).map(sessionImageRef),
    mentions: [],
    model: null,
    requestId: id,
    parentId: null,
  };
}

function emptyBlock(key: string, startedAtMs: number, requestIds: readonly string[] = []): Block {
  return {
    key,
    user: null,
    texts: [],
    cards: [],
    tasks: [],
    rows: [],
    orchestratorSteps: [],
    thinking: [],
    steers: [],
    compactions: [],
    seams: [],
    nudges: [],
    taskNotices: [],
    requestIds: requestIds.length > 0 ? [...requestIds] : [key],
    state: "working",
    error: null,
    startedAtMs,
    endedAtMs: null,
    worked: [{ fromMs: startedAtMs, toMs: null }],
    cardWait: false,
  };
}

/**
 * Another session's message (S30): `brigadier send <session>` gives it to this session's CLI
 * under a header naming the sender, `[message from session "‹title›" (‹id›) · sm-…]`.
 */
const FROM_SESSION = /^\[message from session "[^"]*" \(([^)]+)\) · (sm-[0-9a-f]+)\]\n?/;

/**
 * A message the user sent while the session worked (run2-D40): Brigadier gives it to the CLI
 * under a line naming its queue id, `[queued message ‹id› · judge it: …]`, or, when it waited
 * for its own turn, `[queued message ‹id›, now on its own turn]`. The thread hides that line
 * (run2-D44).
 */
const QUEUED = /^\[queued message (\S{8})(?: · [^\]\n]*|, now on its own turn)\]\n?/;

/** What the session hears of a queued message the user deleted once it had it: never drawn. */
const WITHDRAWN = /\n*\[queued message \S{8} withdrawn: [^\]\n]*\]\n*/g;

/** The queue id a user message's frame names, if it has one. */
function queuedId(text: string): string | null {
  return QUEUED.exec(text)?.[1] ?? null;
}

/** Whether a user message is only Brigadier's word that a queued message was taken back. */
function onlyWithdrawn(text: string): boolean {
  return text.trim() !== "" && text.replace(WITHDRAWN, "").trim() === "";
}

/** A user message as the thread shows it: another session's when its header says so, its body without the header. */
function fromSession(message: Message, authored = false): { message: Message; text: string } {
  if (authored) return { message, text: message.text };
  const queued = QUEUED.exec(message.text);
  const body = message.text.slice(queued?.[0].length ?? 0).replace(WITHDRAWN, "\n").trim();
  if (queued || body !== message.text.trim()) message = { ...message, text: body };
  const header = FROM_SESSION.exec(message.text);
  if (!header) return { message, text: message.text };
  const text = message.text.slice(header[0].length);
  return { message: { ...message, id: header[2]!, role: "peer", text, peer: { messageId: header[2]!, from: header[1]! } }, text };
}

/**
 * Brigadier's one message of what a session's workers did (S29), `[worker events wn-…] Worker
 * "‹title›" (‹id›): ‹what›; … (ev-…, …)`: a quiet line per worker in plain words, without its
 * ids, or null when the message is not one.
 */
export function workerNotices(text: string): string[] | null {
  const header = /^\[worker events wn-[0-9a-f]+\] /.exec(text);
  if (!header) return null;
  const body = text.slice(header[0].length).replace(/ \((?:ev-[0-9a-f]+(?:, )?)+\)\s*$/, "");
  return body.split(/; (?=Worker ")/).map((line) => line.replace(/^Worker "(.*?)" \([^)]*\): /, (_, title: string) => (title ? `Worker "${title}" ` : "A worker ")));
}

/**
 * The CLI's words of a background job's end in a few plain ones: Claude's `Background command
 * "pnpm test" completed (exit code 0)` and Brigadier's `[your background command `…` finished
 * after your turn · exit 0]` read "Background command finished · exit 0" (D21). Words it doesn't
 * know stand as they are.
 */
export function wakeWords(text: string): string {
  const codex = /^\[your background command `[\s\S]*` finished after your turn · ([^\]\n]+)\]/.exec(text);
  if (codex) {
    const how = codex[1]!.trim();
    const exit = /^exit (-?\d+)$/.exec(how)?.[1];
    if (exit !== undefined) return `Background command ${exit === "0" ? "finished" : "failed"} · exit ${exit}`;
    return `Background command ${how === "completed" ? "finished" : how}`;
  }
  const said = /^(.+?) "[\s\S]*" (completed|failed|was stopped|was killed|stopped|killed)\b(?:[^\n]*?exit code (-?\d+))?/.exec(text.trim());
  if (!said) return text;
  const [, what, how, exit] = said;
  const word = how === "completed" ? (exit !== undefined && exit !== "0" ? "failed" : "finished") : how === "failed" ? "failed" : "stopped";
  return exit === undefined ? `${what} ${word}` : `${what} ${word} · exit ${exit}`;
}

/** Whether an entry starts a turn: the user's own message in the session's thread. */
function startsTurn(entry: FileEntry): boolean {
  return entry.event.type === "message" && entry.event.role === "user" && !entry.parent && !entry.steer;
}

/**
 * A turn's state, start and end set on its block, from how the CLI ended it. `earlier`: the
 * block's turns before it (Brigadier's word of its workers joined it): it started with the
 * first, and worked in each.
 */
function settle(block: Block, turn: ThreadTurn | undefined, earlier: readonly ThreadTurn[] = []): void {
  const { state, error } = turnState(turn ?? { end: null, error: null });
  const first = earlier[0] ? (earlier[0].startedAtMs ?? block.startedAtMs) : null;
  const own = turn?.startedAtMs ?? first ?? block.startedAtMs;
  const endedAtMs = turn?.end ? (turn.endedAtMs ?? own) : null;
  const before = earlier.map((each) => {
    const fromMs = each.startedAtMs ?? first ?? own;
    return { fromMs, toMs: each.endedAtMs ?? fromMs };
  });
  Object.assign(block, {
    state,
    error,
    startedAtMs: first ?? own,
    endedAtMs,
    worked: [...before, { fromMs: own, toMs: endedAtMs }],
  } satisfies Partial<Block>);
  // A turn over has no reasoning still coming.
  if (endedAtMs !== null) block.thinking = block.thinking.map((segment) => (segment.complete ? segment : { ...segment, complete: true }));
}

/** How far apart the board's and the thread's word of one compaction may be. */
const SAME_COMPACTION_MS = 2000;

/** Whether the thread's compaction `held` is the board's `compaction`: it began or ended within it. */
function sameCompaction(held: BlockCompaction, compaction: BlockCompaction): boolean {
  const within = (atMs: number) => atMs >= compaction.startedAtMs - SAME_COMPACTION_MS && atMs <= (compaction.endedAtMs ?? Number.POSITIVE_INFINITY) + SAME_COMPACTION_MS;
  return within(held.startedAtMs) || within(held.endedAtMs ?? held.startedAtMs);
}

/** Whether anything of the block's own came after `position`. */
function itemsAfter(block: Block, position: number): boolean {
  const lists: readonly (readonly { position: number }[])[] = [block.texts, block.orchestratorSteps, block.rows, block.cards, block.steers, block.taskNotices, block.thinking];
  return lists.some((items) => items.some((item) => item.position > position));
}

/**
 * Brigadier's own items in the turns: each in the turn that carried its request (or, a request
 * no turn names, the turn it came in), after the CLI's items from before it.
 */
function place(blocks: readonly Block[], times: readonly (readonly [number, number])[][], board: BoardDigest): void {
  const byRequest = new Map<string, number>();
  blocks.forEach((block, index) => {
    for (const id of block.requestIds) byRequest.set(id, index);
  });
  const taken = new Map<string, number>();
  for (const item of boardItems(board)) {
    const named = item.requestId === null ? undefined : byRequest.get(item.requestId);
    const index = named ?? blocks.findLastIndex((block) => block.startedAtMs <= item.atMs);
    const block = blocks[index];
    const known = times[index];
    if (!block || !known) continue;
    const before = known.filter(([, atMs]) => atMs <= item.atMs);
    const after = (before.at(-1) ?? known[0] ?? [0])[0];
    // Items between the same two of the CLI's keep the board's order.
    const slot = `${index}:${after}`;
    const nth = taken.get(slot) ?? 0;
    taken.set(slot, nth + 1);
    const position = after + 0.5 + nth / 1000;
    switch (item.kind) {
      case "task":
        block.tasks.push(item.id);
        break;
      case "row":
        block.rows.push({ ...item.row, position });
        break;
      case "orchestrator":
        block.orchestratorSteps.push({ ...item.step, position });
        break;
      case "compaction":
        // A Chat's compaction its CLI said in the thread too is that one divider, not a second.
        if (!blocks.some((each) => each.compactions.some((held) => sameCompaction(held, item.compaction)))) block.compactions.push({ ...item.compaction, position });
        break;
      case "card":
        block.cards.push({ ...item.card, position });
        // It waits for the answer to its own question card.
        if (item.card.type === "question" && board.questions[item.card.id]?.answer === null) block.cardWait = true;
        break;
    }
  }
}

/**
 * A thread's turns as blocks, oldest first, with Brigadier's own items placed in them
 * (`board`). Subagent lifecycle calls and their child items stay in their strip and History;
 * their source entries are retained for those views. `session`: the thread is a session's, where
 * Brigadier compacts a long one itself. `typed`: the words of the user's own messages that no
 * turn read names yet.
 */
export function cliBlocks(view: ThreadView, conversationId: string, board?: BoardDigest, session = false, typed: readonly string[] = []): Block[] {
  const blocks: Block[] = [];
  // Each block's CLI items, by position and time.
  const times: [number, number][][] = [];
  let block: Block | null = null;
  // The turn the entries are in, by its place in `view.turns`, and each block's turn: a turn the
  // CLI begins with a notice inside Brigadier's own turn goes on that turn's block, not a block of its own.
  let turn = -1;
  const turnOf = new Map<Block, ThreadTurn | undefined>();
  // A block's turns before its last: Brigadier's word of what its workers did joins the block
  // that started them, as one more turn of it.
  const joined = new Map<Block, ThreadTurn[]>();
  const close = (each: Block) => settle(each, turnOf.get(each), joined.get(each));
  // The turn at `position` goes on with `each`, one more turn of it: its turn before joined it.
  const join = (each: Block, position: number, atMs: number | undefined) => {
    const last = turnOf.get(each);
    joined.set(each, [...(joined.get(each) ?? []), ...(last ? [last] : [])]);
    turnOf.set(each, view.turns[turn]);
    for (const request of view.turns[turn]?.requests ?? []) if (!each.requestIds.includes(request)) each.requestIds.push(request);
    if (atMs !== undefined) times.at(-1)?.push([position, atMs]);
  };
  // A turn the user's message started while the block before still held (its workers or its
  // CLI's background commands still ran) goes on in that block, as the daemon says.
  const goesOn = (each: Block | null): each is Block => !!each && !internal.has(each) && !!view.turns[turn]?.joins;
  // Whether Brigadier's word of its workers (`text`) names one started in `each`'s turns.
  const startedIn = (each: Block, text: string): boolean => {
    const from = (joined.get(each)?.[0] ?? turnOf.get(each))?.startedAtMs ?? each.startedAtMs;
    const named = [...text.matchAll(/Worker ".*?" \(([^)\s]+)\): /g)].map((match) => match[1]!);
    return named.some((id) => (view.workers ?? []).some((row) => (row.id === id || row.id.endsWith(id)) && row.startedAtMs >= from));
  };
  const internal = new Set<Block>();
  const waited = waitOutputs(view);
  // The turns, by place, the CLI told a background job's end in: Claude tells it in the next turn
  // it runs, which may be Brigadier's own `/compact`.
  const noticed = new Set<number>();
  view.entries.reduce((at, entry) => {
    const next = !entry.parent && startsTurn(entry) ? at + 1 : at;
    if (!entry.parent && entry.event.type === "taskNotice") noticed.add(next);
    return next;
  }, -1);
  let maintenance: "turn" | "steered" | "upkeep" | null = null;
  const seams = view.seams ?? [];
  const nudges = new Set(seams.flatMap((seam) => (seam.nudge ? [seam.nudge] : [])));
  // A successor that started by itself: its first message is Brigadier's word to carry on.
  const carried = new Set(seams.flatMap((seam) => (seam.carried && seam.at ? [seam.at] : [])));
  // A queued message shows once (run2-D44): in the rail while the CLI has it there, else
  // where the CLI last took it (a message that waited for its own turn was read before).
  const inRail = new Set((board?.queueItems ?? []).flatMap((item) => (item.delivered ? [item.id.slice(-8)] : [])));
  const lastTaken = new Map<string, number>();
  view.entries.forEach((entry, position) => {
    const id = !entry.parent && entry.said?.by !== "user" && entry.event.type === "message" && entry.event.role === "user" ? queuedId(entry.event.text) : null;
    if (id) lastTaken.set(id, position);
  });
  // The steers that name no queued message answer the turn's other requests, in order.
  const framed = (request: string) => lastTaken.has(request.slice(-8));
  view.entries.forEach((entry, position) => {
    if (entry.parent) return;
    if (startsTurn(entry)) turn += 1;
    let { event } = entry;
    // A successor's first message: the hand-off, then the user's words, if any.
    const handoff = entry.said?.by !== "user" && event.type === "message" && event.role === "user" && event.text.trimStart().startsWith(HANDOFF_MARK);
    if (handoff && event.type === "message") {
      const end = event.text.indexOf(HANDOFF_END);
      if (end >= 0) event = { ...event, text: event.text.slice(end + HANDOFF_END.length).trimStart() };
    }
    const asked = event.type === "message" && event.role === "user" ? event : null;
    // Who wrote it, as Brigadier noted when it sent it to the CLI (L12): its own words (a note,
    // a nudge, a frame, its `/compact`) are never a bubble, and of the user's words with its
    // own around them only the user's show. Peer deliveries keep their sender and authored
    // content. Of a message it has no note of (sent before it
    // noted them) its words say whose it is, the way they begin.
    const said = asked ? entry.said : undefined;
    const ours = said?.by === "brigadier";
    const theirs = said?.by === "user" || said?.by === "both";
    // Brigadier's own turn carries no request of the user's: what the user sent shows as ever.
    const brigadiers = startsTurn(entry) && view.turns[turn]?.requests.length === 0;
    // The hand-off is under the hood (run2-D44): Brigadier's word to write the note, the turn
    // it starts, and from the ledger's upkeep on, the rest of a turn it was steered into; the
    // work the CLI finishes first still shows. A notice the CLI starts a turn with goes on it.
    if (asked) {
      const text = asked.text.trimStart();
      const nudge = nudges.has(asked.itemId) || (!theirs && text.startsWith(NUDGE_MARK));
      if (startsTurn(entry) && !(maintenance === "turn" && brigadiers && CLI_NOTICE.test(text))) maintenance = null;
      if (nudge) {
        if (startsTurn(entry)) {
          maintenance = "turn";
          if (block) close(block);
          block = emptyBlock(asked.itemId, entry.atMs ?? 0, []);
          blocks.push(block);
          turnOf.set(block, view.turns[turn]);
          internal.add(block);
          times.push([[position, entry.atMs ?? 0]]);
        } else maintenance = "steered";
        return;
      }
    } else if (maintenance === "steered" && upkeep(event)) maintenance = "upkeep";
    if (maintenance === "turn" || (maintenance === "upkeep" && !asked)) {
      if (block && startsTurn(entry)) turnOf.set(block, view.turns[turn]);
      return;
    }
    // Brigadier's message of what the workers did is its quiet lines in the turn, the same way.
    const news = asked && !theirs && workerNotices(asked.text);
    const queued = asked && said?.by !== "user" && queuedId(asked.text);
    const unshown =
      !!asked && ((!!queued && (inRail.has(queued) || lastTaken.get(queued) !== position)) || (!said && onlyWithdrawn(asked.text)));
    // A long session's `/compact` is Brigadier's (run2-D5): the thread shows the compaction's
    // divider, never the command or what its summary must keep, from the stream's first word of
    // it on. Its checkpoint request before Codex compacts is Brigadier's too (run2-D9). Its turn
    // names no request of the user's own: none, or a block's before it that a background
    // command's end went on with. Not read yet, it names none: the user's own `/compact` is
    // one of the words they sent.
    const read = view.turns[turn];
    const compacting =
      !!asked &&
      startsTurn(entry) &&
      (/^\/compact(\s|$)/.test(asked.text) || asked.text.startsWith(CHECKPOINT_MARK)) &&
      (said
        ? ours
        : session &&
          (read
            ? read.requests.every((request) => blocks.some((earlier) => earlier.requestIds.includes(request)))
            : !typed.includes(asked.text.trim())));
    // Brigadier's word to a session's CLI is no bubble (run2-D44): its own turn's, or a turn
    // of a request already shown (a background command's end), or steered in.
    const carries = view.turns[turn]?.requests ?? [];
    const shownBefore = carries.length > 0 && carries.every((request) => blocks.some((earlier) => earlier.user?.kind === "message" && earlier.user.message.id === request));
    // Peer messages retain their authored content and sender; only automatic engine notices hide.
    const peer = !!asked && FROM_SESSION.test(asked.text);
    const says =
      !!asked && (said && !peer ? ours : !theirs && BRIGADIER_SAYS.test(asked.text) && (!startsTurn(entry) || brigadiers || shownBefore));
    // A successor that carried on by itself: its first message is Brigadier's, the hand-off and
    // its word to carry on, no bubble; the successor's work shows.
    const own = !!asked && (carried.has(asked.itemId) || (!said && brigadiers && (handoff || asked.text.trim() === CARRY_ON)));
    // Brigadier's word that a Codex command ended after its turn: its one plain line (D21).
    const woke = !!asked && says && CODEX_WAKE.test(asked.text);
    const before = block;
    if (asked && (own || compacting || news || unshown || says)) {
      // Brigadier's word of what workers did that the block before started, or that a command
      // it started ended (D21): a turn more of that block, never one of its own (a message the
      // user typed still starts one).
      const joins =
        startsTurn(entry) &&
        !!before &&
        !internal.has(before) &&
        ((!!news && startedIn(before, asked.text)) ||
          (woke && carries.some((request) => before.requestIds.includes(request))) ||
          // Brigadier's `/compact` that Claude told a background command's end in: one more turn
          // of the block before, as Codex's word of its command's end is (D44, D52).
          (compacting && noticed.has(turn)) ||
          // A queued message still in the rail that woke the session to judge it while the
          // block before held: that block's, no bubble until it joins.
          goesOn(before));
      if (joins && block) {
        join(block, position, entry.atMs);
      } else if (startsTurn(entry)) {
        if (block) close(block);
        block = emptyBlock(asked.itemId, entry.atMs ?? 0, view.turns[turn]?.requests ?? []);
        blocks.push(block);
        turnOf.set(block, view.turns[turn]);
        times.push([[position, entry.atMs ?? 0]]);
        // Brigadier's compaction is done under the hood: its turn is no block (no header, no
        // time, nothing cut off); its divider goes after the block before.
        if (compacting) internal.add(block);
      }
      if (entry.atMs !== undefined && !startsTurn(entry)) times.at(-1)?.push([position, entry.atMs]);
      if (news) news.forEach((text, nth) => block?.taskNotices.push({ position: position + nth / 1000, id: `${asked.itemId}:${nth}`, text, ...(joins && { inWork: true }) }));
      if (woke && !waitEnded(asked.text, view)) block?.taskNotices.push({ position, id: asked.itemId, text: wakeWords(asked.text), detail: asked.text, inWork: true });
      return;
    }
    if (startsTurn(entry) && event.type === "message") {
      const requests = view.turns[turn]?.requests ?? [];
      const atMs = entry.atMs ?? 0;
      // A turn of a request an earlier turn carried (a background command's end, told after its
      // turn) starts with the CLI's message, not the user's: it keeps its own id. A queued message
      // the CLI began a turn with is its own request, which the turn names only once it joined:
      // the request before it may still lead the turn's list.
      const first = requests[0];
      const named = said?.by === "user" ? null : queuedId(event.text);
      const id = named
        ? (requests.find((request) => request.endsWith(named)) ?? `queued-${named}`)
        : first !== undefined && !blocks.some((earlier) => earlier.user?.kind === "message" && earlier.user.message.id === first)
          ? first
          : event.itemId;
      const shown = fromSession(userMessage(conversationId, entry, id, said?.by === "both" ? said.user : event.text, position + 1), theirs);
      // Sent while the block before still held: its bubble goes in that block, where it came,
      // and the turn's work goes on there.
      if (goesOn(block)) {
        join(block, position, entry.atMs);
        block.steers.push({ ...shown, position });
        return;
      }
      if (block) close(block);
      block = emptyBlock(event.itemId, atMs, requests);
      block.user = shown.message.role === "peer" ? { kind: "peer", ...shown, earlier: false } : { kind: "message", ...shown };
      blocks.push(block);
      turnOf.set(block, view.turns[turn]);
      times.push([[position, atMs]]);
      return;
    }
    // Anything before the first user message belongs to no turn.
    if (!block) return;
    const current: Block = block;
    if (entry.atMs !== undefined) times.at(-1)?.push([position, entry.atMs]);
    switch (event.type) {
      case "message":
      case "messageDelta": {
        if (event.type === "message" && event.role === "user") {
          // A steer answers its own request, the next the turn names; a queued message's is
          // its own id (the request it joined as).
          const named = said?.by === "user" ? null : queuedId(event.text);
          const taken = new Set(current.steers.map((steer) => steer.message.id));
          const id = named
            ? (current.requestIds.find((request) => request.endsWith(named)) ?? `queued-${named}`)
            : (current.requestIds.slice(1).find((request) => !taken.has(request) && !framed(request)) ?? event.itemId);
          current.steers.push({ ...fromSession(userMessage(conversationId, entry, id, said?.by === "both" ? said.user : event.text, position + 1), theirs), position });
          return;
        }
        if (event.type === "message" && event.role !== "assistant") return;
        // A reply still streaming shows its finished lines only, none until its first (D9 b).
        const streaming = event.type === "messageDelta";
        const text = shownText(event.text, streaming);
        // A reply with no words is none: assistant-ui drops a blank part, and the turn's parts
        // then count fewer than its texts (Codex's empty last reply after a send-back, D58).
        if (!text.trim()) return;
        // What the reply is, as the stream said it, else as the file keeps it (Codex's `phase`).
        const phase = (event.type === "messageDelta" ? event.phase : undefined) ?? entry.phase;
        current.texts.push({
          messageId: event.itemId,
          position,
          text,
          model: null,
          update: isUpdate(text),
          ...(phase === "answer" && entry.atMs !== undefined && { answerAtMs: entry.atMs }),
          ...(phase === "commentary" && { commentary: true }),
          ...(entry.atMs !== undefined && { atMs: entry.atMs }),
        });
        return;
      }
      case "reasoning":
      case "reasoningDelta": {
        if (!event.text.trim()) return;
        const atMs = entry.atMs ?? current.startedAtMs;
        current.thinking.push({
          itemId: event.itemId,
          requestId: current.key,
          text: event.text,
          position,
          startedAtMs: atMs,
          updatedAtMs: entry.updatedAtMs ?? atMs,
          throughPosition: position,
          complete: event.type === "reasoning",
        });
        return;
      }
      // The CLI's line for a background job's end: one plain line in the work the turn it starts
      // goes on with ("Background command finished · exit 0"), its own words on hover (D21).
      case "taskNotice":
        if (waitEnded(event.text, view) || subagentNotice(event.text)) return;
        current.taskNotices.push({ position, id: event.itemId, text: wakeWords(event.text), detail: event.text, inWork: true });
        return;
      // A compaction is one divider: the stream's word that it runs (no file keeps it) is the
      // divider its end then takes over, in place. Said again with nothing between (the file's
      // word and the stream's while the turn is open), it is the same one.
      case "compactionStarted":
      case "compactionEnded": {
        const target = internal.has(current) ? blocks.findLast((each) => !internal.has(each)) : current;
        if (!target) return;
        const atMs = entry.atMs ?? target.startedAtMs;
        const last = target.compactions.at(-1);
        const again = !!last && !itemsAfter(target, last.position);
        if (event.type === "compactionStarted") {
          if (again) return;
          target.compactions.push({ id: `compaction:${position}`, inTurn: true, automatic: event.automatic, state: "running", error: null, position, startedAtMs: atMs, endedAtMs: null });
          return;
        }
        const state = event.error === null ? "done" : "failed";
        if (again && last.state === "running") Object.assign(last, { state, error: event.error, endedAtMs: atMs });
        else if (!again) target.compactions.push({ id: `compaction:${position}`, inTurn: true, automatic: event.automatic, state, error: event.error, position, startedAtMs: atMs, endedAtMs: atMs });
        return;
      }
      default: {
        if (readsOnly(event, waited)) return;
        const tool = keptToolStep(entry, turnOf.get(current) ?? null, view.background);
        // Its `brigadier send` to another session is the row of the message it sent (S30).
        const kind = sessionSent(tool, event, board) ?? tool;
        if (kind) current.orchestratorSteps.push({ kind, position, ...(entry.atMs !== undefined && { atMs: entry.atMs }), requestId: current.key });
      }
    }
  });
  if (block) close(block);
  // A compaction its turn ended with (Brigadier's at a turn's end, or the CLI's after its last
  // reply) is between that turn and the next: after the block, not in its work.
  for (const each of blocks) {
    if (internal.has(each)) continue;
    for (const compaction of each.compactions) {
      if (compaction.inTurn && !itemsAfter(each, compaction.position) && (!isLive(each.state) || !compaction.automatic)) compaction.inTurn = false;
    }
  }
  if (board) {
    place(blocks, times, board);
    dropShownCalls(blocks, board);
  }
  // Each seam's divider after the turn before its successor's first message, or after the last
  // turn while the successor is still to come.
  for (const seam of seams) {
    if (seam.open) continue;
    const next = seam.at === null ? blocks.length : blocks.findIndex((each) => each.key === seam.at);
    const before = blocks[next < 0 ? -1 : next - 1];
    if (next < 0 || !before) continue;
    before.seams.push({ n: seam.n, note: seam.note, start: seam.start, death: seam.death, movedTo: seam.movedTo });
  }
  return blocks.filter((each) => !internal.has(each));
}

/** A thread not read yet: no turns. */
const NOT_READ: ThreadView = { entries: [], turns: [] };

/**
 * A thread whose view couldn't be read, from the messages Brigadier keeps: those stored before
 * `before` (`unreadSince`) as turns that ended, each user message one with the replies after it.
 * One stored since is in none, so it stays on its way (`cliThreadTree`).
 */
export function keptThread(messages: readonly Message[], fullText: Readonly<Record<string, string>>, before: number): ThreadView {
  const entries: FileEntry[] = [];
  const turns: ThreadTurn[] = [];
  for (const message of messages) {
    if ((message.role !== "user" && message.role !== "assistant") || message.createdAtMs >= before) continue;
    const atMs = message.createdAtMs;
    if (message.role === "user") {
      turns.push({ userMessage: message.id, startedAtMs: atMs, endedAtMs: atMs, end: "done", error: null, requests: [message.requestId ?? message.id] });
    } else if (turns.length === 0) continue;
    const last = turns.at(-1)!;
    last.endedAtMs = atMs;
    entries.push({ event: { type: "message", itemId: message.id, role: message.role, text: fullText[message.id] ?? message.text }, atMs });
  }
  return { entries, turns };
}

/** How a message on its way reads: working while its turn starts, failed when the CLI never took it. */
type RunNow = { state: RunState; error: string | null };

/** A block of a message sent but not in the CLI's file yet: the bubble, and a block that works on it. */
function onItsWay(key: string, user: Block["user"], startedAtMs: number, run: RunNow): Block {
  const block = { ...emptyBlock(key, startedAtMs), user };
  if (run.state !== "failed") return block;
  // It never reached the CLI: the run says why.
  return {
    ...block,
    state: "failed",
    error: run.error ?? "The CLI did not take the message.",
    endedAtMs: startedAtMs,
    worked: [{ fromMs: startedAtMs, toMs: startedAtMs }],
  };
}

/**
 * The thread as assistant-ui's message tree: the CLI's turns, one after another, then the
 * messages still on their way: sent but not stored (`pending`), and stored but in no turn of
 * the CLI's yet (a user message newer than the newest the CLI has, that no turn carried).
 * `thread` null: not read yet, and with no view since `unreadSince`. Which stored messages a
 * turn carries is unknown until it is read, so only one stored since then is on its way (one
 * just sent): each earlier turn's message would read as one working since it was sent.
 */
export function cliThreadTree(
  thread: ThreadView | null,
  conversationId: string,
  messages: readonly Message[],
  fullText: Readonly<Record<string, string>>,
  pending: readonly PendingMessage[],
  board: BoardDigest,
  run: RunNow,
  session = false,
  unreadSince = Number.POSITIVE_INFINITY,
): ThreadTree {
  const view = thread ?? NOT_READ;
  const stored = thread ? messages : messages.filter((message) => message.createdAtMs >= unreadSince);
  const named = new Set(view.turns.flatMap((turn) => turn.requests));
  const typed = stored.flatMap((message) => (message.role === "user" && !named.has(message.requestId ?? message.id) ? [(fullText[message.id] ?? message.text).trim()] : []));
  const blocks = cliBlocks(view, conversationId, board, session, typed);
  // The user's own messages as Brigadier keeps them, by their request: a turn that names one
  // shows it as it was sent, its pasted chips, mentions and attachments with it.
  const own = new Map(stored.flatMap((message) => (message.role === "user" ? [[message.requestId ?? message.id, message] as const] : [])));
  // A queued message no turn names yet (`queued-‹id›`) is kept under its queue id.
  const keptOf = (id: string): Message | undefined =>
    own.get(id) ?? (id.startsWith("queued-") ? [...own.entries()].find(([request]) => request.endsWith(id.slice("queued-".length)))?.[1] : undefined);
  const asSent = (message: Message, text: string): { message: Message; text: string } => {
    const kept = keptOf(message.id);
    return kept ? { message: kept, text: fullText[kept.id] ?? kept.text } : { message, text };
  };
  // The user's messages a turn already shows: none of them is on its way as well.
  const drawn = new Set<string>();
  for (const block of blocks) {
    if (block.user?.kind === "message") block.user = { kind: "message", ...asSent(block.user.message, block.user.text) };
    block.steers = block.steers.map((steer) => (steer.message.role === "peer" ? steer : { ...steer, ...asSent(steer.message, steer.text) }));
    for (const said of [...(block.user?.kind === "message" ? [block.user] : []), ...block.steers]) drawn.add(said.message.requestId ?? said.message.id);
  }
  // A turn whose message the stream said before the turns were read again names no request yet:
  // its message is the CLI's, under the CLI's id. It is the one Brigadier kept since the turn
  // before began that no turn carries, so its bubble stays the one shown while it was on its way,
  // not mounted afresh under another id (the view thrown off the sent turn's place) and again
  // once the turns are read.
  const read = new Set(view.turns.map((turn) => turn.userMessage));
  blocks.forEach((block, index) => {
    const said = block.user;
    if (said?.kind !== "message" || said.message.id !== block.key || read.has(block.key)) return;
    const after = blocks[index - 1]?.startedAtMs ?? Number.NEGATIVE_INFINITY;
    const sent = stored.find((message) => {
      const request = message.requestId ?? message.id;
      return message.role === "user" && message.createdAtMs > after && message.createdAtMs <= block.startedAtMs && !named.has(request) && !drawn.has(request);
    });
    if (!sent) return;
    block.user = { kind: "message", message: sent, text: fullText[sent.id] ?? sent.text };
    block.requestIds = [sent.requestId ?? sent.id];
    drawn.add(sent.requestId ?? sent.id);
  });
  // A queued message joins the turn that read it after the CLI had it (the turn names it only
  // once it ends): it is drawn there, not on its way too.
  const carried = new Set([...named, ...drawn]);
  const heard = Math.max(
    0,
    ...view.entries.flatMap((entry) => (entry.event.type === "message" && entry.event.role === "user" && !entry.parent ? [entry.atMs ?? 0] : [])),
  );
  for (const message of stored) {
    if (message.role !== "user" || message.createdAtMs <= heard || carried.has(message.requestId ?? message.id)) continue;
    const text = fullText[message.id] ?? message.text;
    blocks.push(onItsWay(message.id, { kind: "message", message, text }, message.createdAtMs, run));
  }
  for (const entry of pending) blocks.push(onItsWay(entry.localId, { kind: "pending", pending: entry }, entry.createdAtMs, { state: "running", error: null }));

  const nodes: ThreadNode[] = [];
  let parent: string | null = null;
  for (const block of blocks) {
    if (block.user) {
      // A message sent from here keeps the id it showed under while pending (`shownIdOf`), so
      // its bubble isn't mounted afresh when it is stored and its turn is placed once it shows.
      const id = block.user.kind === "pending" ? block.user.pending.localId : shownIdOf(block.user.message.id);
      nodes.push({ id, parentId: parent, kind: "user", block, head: null });
      parent = id;
    }
    if (!blockShows(block)) continue;
    const id = `request:${block.key}`;
    nodes.push({ id, parentId: parent, kind: "block", block, head: null });
    parent = id;
  }
  return { nodes, headId: parent };
}
