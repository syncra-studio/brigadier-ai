import { agentDetail, brigadierCalls, brigadierUnshown, isPlumbing, toolName, type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import { backgroundOf } from "@/state/nativeWorkers";
import {
  type Block,
  type BlockCompaction,
  type BlockOrchestratorStep,
  type BoardDigest,
  type BlockState,
  blockShows,
  boardItems,
  isLive,
  isUpdate,
  type ThreadNode,
  type ThreadTree,
} from "@/app/conversation/blocks";
import { attachReplies, sessionSent, sessionWait } from "@/app/conversation/sessionSend";
import { INTERRUPTED } from "@/app/conversation/turnError";
import { coreHolds } from "@/app/conversation/turnWorkers";
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
  Wake,
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
 * A result's text: a JSON content array's text blocks, one per line, else the result as it is
 * (`digest::content_text`). Adapters send an MCP result as its text; only records written before
 * they did hold the array.
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

export function stepInTurn(step: ToolStep | null, turn: Pick<ThreadTurn, "end" | "endedAtMs"> | null, task: BackgroundTask | undefined): ToolStep | null {
  if (!step) return null;
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

/** The worker actions whose own line says the `brigadier` call that made it (D150). */
const ACTION_LINE: Record<string, BlockOrchestratorStep["kind"]["type"]> = { pause: "paused", resume: "resumed", close: "stopped", switch: "providerTransition" };

/**
 * A `brigadier send` whose message has its own "Messaged …" row (a worker's, a session's), a
 * `brigadier ask` whose card has its own "Asking questions" row, and a `brigadier pause`,
 * `resume`, `close` or `switch` whose worker has its own "Paused …", "Resumed …", "Stopped …"
 * or switch line, is no row of its own: the two said the same (D47, D58, D150).
 */
function dropShownCalls(blocks: readonly Block[], board: BoardDigest): void {
  const held = heldMessages(blocks);
  for (const block of blocks) {
    const sent = block.orchestratorSteps.flatMap((step) => ((step.kind.type === "messaged" || step.kind.type === "peerSent") && step.atMs !== undefined ? [step.atMs] : []));
    const asked = block.cards.flatMap((card) => {
      const question = card.type === "question" ? board.questions[card.id] : undefined;
      return question?.kind.type === "orchestrator" ? [question.createdAtMs] : [];
    });
    const lines = (type: string) => block.orchestratorSteps.flatMap((step) => (step.kind.type === type && step.atMs !== undefined ? [step.atMs] : []));
    block.orchestratorSteps = block.orchestratorSteps.filter((step) => {
      const { kind, atMs } = step;
      if (held.has(step)) return false;
      if (kind.type !== "tool" || kind.item !== "command" || kind.detail === null || atMs === undefined) return true;
      const sub = brigadierCalls(kind.detail)[0] ?? "";
      const line = ACTION_LINE[sub];
      const shown = sub === "send" ? sent : sub === "ask" ? asked : line ? lines(line) : [];
      const [from, to] = [atMs - SEND_SLACK_MS, (kind.endedAtMs ?? atMs) + SEND_SLACK_MS];
      return !shown.some((at) => at >= from && at <= to);
    });
  }
}

/**
 * The thread's messages to a worker it paused: each is held for, and goes with, its resume, which
 * the "Resumed …" line says; no "Messaged …" row of its own (D7, D150). The user's own keep theirs.
 */
function heldMessages(blocks: readonly Block[]): Set<BlockOrchestratorStep> {
  const paused = new Map<string, boolean>();
  const held = new Set<BlockOrchestratorStep>();
  const steps = blocks.flatMap((block) => block.orchestratorSteps).filter((step) => step.atMs !== undefined).toSorted((a, b) => a.atMs! - b.atMs!);
  for (const step of steps) {
    const { kind } = step;
    if (kind.type === "paused" || kind.type === "resumed" || kind.type === "stopped") paused.set(kind.taskId, kind.type === "paused");
    else if (kind.type === "messaged" && !kind.byUser && paused.get(kind.taskId)) held.add(step);
  }
  return held;
}

/**
 * Whether a background job's end is that of the thread's own `brigadier wait` (its notice may
 * name the task by its description, not its command): no line, as the wait had no row (D38,
 * D47); the worker rows after it say what happened.
 */
function waitEnded(wake: Wake, view: ThreadView): boolean {
  const task = view.background?.find((each) => each.label === wake.label);
  return [wake.command, wake.label, task?.command].some((words) => words != null && brigadierCalls(words)[0] === "wait");
}

/**
 * Where Claude writes a background command's output, as its call's result says: after a reload
 * the result is all that names it (`BackgroundTask.outputFile` is live only).
 */
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
    // A limit cut it and its work goes on on another account or CLI: never a failure (D180).
    case "handedOver":
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

/** Another session's message as its words, without the header naming who sent it (goal 4). */
export function peerWords(text: string): string {
  return text.replace(FROM_SESSION, "");
}

/**
 * A message the user sent while the session worked (run2-D40): Brigadier gives it to the CLI
 * under a line naming its queue id, `[queued message ‹id› · judge it: …]`, or, when it waited
 * for its own turn, `[queued message ‹id›, now on its own turn]`, or, steered in by the user,
 * `[queued message ‹id›, steered in by the user: …]`. The thread hides that line (run2-D44).
 */
const QUEUED = /^\[queued message (\S{8})(?: · |, )[^\]\n]*\]\n?/;

/** The line a message the user steered in while the session worked goes under (D25): hidden. */
const STEERED = /^\[steered in by the user: [^\]\n]*\]\n?/;

/**
 * What the session hears of a queued message it already had that the user deleted, or steered
 * in while it judged it: never drawn.
 */
const WITHDRAWN = /\n*\[queued message \S{8} (?:withdrawn|steered in by the user): [^\]\n]*\]\n*/g;

/** A successor's first message without its hand-off: the user's words after it, if any. */
function afterHandoff(text: string): string {
  const end = text.trimStart().startsWith(HANDOFF_MARK) ? text.indexOf(HANDOFF_END) : -1;
  return end >= 0 ? text.slice(end + HANDOFF_END.length).trimStart() : text;
}

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
  const queued = QUEUED.exec(message.text) ?? STEERED.exec(message.text);
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

/** A background job's end in a few plain words: "Background command finished · exit 0" (D21). */
export function wakeWords({ job, end, exit }: Wake): string {
  const what = job === "agent" ? "Agent" : "Background command";
  return exit === null ? `${what} ${end}` : `${what} ${end} · exit ${exit}`;
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
    const byTime = blocks.findLastIndex((block) => block.startedAtMs <= item.atMs);
    // The thread's own question asked in a turn that carries no request (a successor's carry-on
    // after a seam) is filed under the latest request, whose turn was over: it is the turn that
    // asked it that waits on it (D161).
    const askedLater =
      named !== undefined &&
      byTime > named &&
      item.kind === "card" &&
      board.questions[item.card.id]?.kind.type === "orchestrator" &&
      (blocks[named]?.endedAtMs ?? Number.POSITIVE_INFINITY) <= item.atMs;
    const index = named === undefined || askedLater ? byTime : named;
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
 * (`board`). A subagent is its call's row; its own items (`parent`) are not drawn: that row opens
 * them (D96). `session`: the thread is a session's, where
 * Brigadier compacts a long one itself. `typed`: the words of the user's own messages that no
 * turn read names yet.
 */
export function cliBlocks(
  view: ThreadView,
  conversationId: string,
  board?: BoardDigest,
  session = false,
  typed: readonly string[] = [],
  kept: readonly string[] = [],
): Block[] {
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
  // A turn a usage limit ended while its request still works is switching (goal 1): it reads
  // working, not stopped with an error, until its carry-on goes on in the block.
  const switching = (read: ThreadTurn | undefined): ThreadTurn | undefined =>
    read && (read.errorKind === "usageLimit" || read.limit) && read.requests.some((request) => (board?.requests?.[request] as { state?: { type?: string } } | undefined)?.state?.type === "working")
      ? { ...read, end: null, endedAtMs: null, error: null }
      : read;
  const close = (each: Block) => settle(each, switching(turnOf.get(each)), joined.get(each));
  // The turn at `position` goes on with `each`, one more turn of it: its turn before joined it.
  // The block each message that joined one went on in (a seam's successor may start in one).
  const joinedBy = new Map<string, Block>();
  const join = (each: Block, position: number, atMs: number | undefined) => {
    const item = view.entries[position]?.event;
    if (item?.type === "message") joinedBy.set(item.itemId, each);
    const last = turnOf.get(each);
    joined.set(each, [...(joined.get(each) ?? []), ...(last ? [last] : [])]);
    turnOf.set(each, view.turns[turn]);
    for (const request of view.turns[turn]?.requests ?? []) if (!each.requestIds.includes(request)) each.requestIds.push(request);
    if (atMs !== undefined) times.at(-1)?.push([position, atMs]);
  };
  // A turn the user's message started while the block before still held (its workers or its
  // CLI's background commands still ran) goes on in that block, as the daemon says.
  // So does a turn begun while the turn before still ran (a CLI's input queued mid-turn, D74).
  // Not read yet while the turn before still runs, it is one the CLI began inside that turn: a
  // turn of the user's starts only once the one before ended (seen live: two headers for 2 s).
  const overlaps = () => {
    const [previous, now] = [view.turns[turn - 1], view.turns[turn]];
    if (!previous) return false;
    if (!now) return previous.end == null;
    return now.startedAtMs != null && now.startedAtMs < (previous.endedAtMs ?? (previous.end == null ? Number.POSITIVE_INFINITY : 0));
  };
  // A turn begun while the core's work block of the block before is open goes on in it (D163):
  // one job, one block, as the core alone says.
  const holds = (each: Block) => coreHolds(view.block, turnOf.get(each)?.endedAtMs ?? null);
  // A turn that carries on the request a usage limit cut (an account or CLI switch) goes on in
  // its block: one block, one switch row, no stopped turn (goal 1), with what the user steered
  // into it meanwhile (D25; seen live). Its requests, else the ones the engine noted it sent: one
  // naming none of the cut turn's, a message the user sent anew, opens its own.
  const carriesOn = (requests: readonly string[] = []) => {
    const before = view.turns[turn - 1];
    if (before?.errorKind !== "usageLimit" && !before?.limit) return false;
    const named = view.turns[turn]?.requests.length ? view.turns[turn]!.requests : requests;
    return named.length === 0 || named.some((request) => before.requests.includes(request));
  };
  // While its turn runs, a turn begun while the block before held goes on in it (D24, D113):
  // the daemon's `joins` may come only once the turn is judged or over.
  const goesOn = (each: Block | null, requests?: readonly string[]): each is Block =>
    !!each && !internal.has(each) && (!!view.turns[turn]?.joins || overlaps() || carriesOn(requests) || (view.turns[turn]?.end == null && holds(each)));
  // Whether Brigadier's word of its workers (`text`) names one started in `each`'s turns.
  const startedIn = (each: Block, text: string): boolean => {
    const from = (joined.get(each)?.[0] ?? turnOf.get(each))?.startedAtMs ?? each.startedAtMs;
    const named = [...text.matchAll(/Worker ".*?" \(([^)\s]+)\): /g)].map((match) => match[1]!);
    // Or one an earlier block started that its requests resumed or messaged back to work (D151).
    const sentBack = (task: string) =>
      (board?.orchestratorSteps ?? []).some((step) => step.requestId !== null && each.requestIds.includes(step.requestId) && (step.kind.type === "resumed" || step.kind.type === "messaged") && step.kind.taskId === task);
    return named.some((id) => (view.workers ?? []).some((row) => (row.id === id || row.id.endsWith(id)) && (row.startedAtMs >= from || sentBack(row.id))));
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
  // where the CLI first read it in the turn that took it (a message that waited for its own
  // turn, or was steered in after it waited, was read before; until the CLI reads it again it is
  // on its way, so it never shows where it was judged and then moves). With no turn naming it
  // yet: where the CLI last read it in the turn running now (it joins that turn), else on its way.
  const inRail = new Set((board?.queueItems ?? []).flatMap((item) => (item.delivered ? [item.id.slice(-8)] : [])));
  // The queued messages the session took in (a turn names them, or Brigadier keeps them as the
  // user's): one handed to a worker, or still judged, never shows in this thread (D68). A turn
  // woken to judge one names it from the start; it took it in only once it is a request here.
  const isRequest = (request: string) => !board?.requests || request in board.requests;
  const tookIn = new Set([...view.turns.flatMap((each) => each.requests.filter(isRequest)), ...kept].map((request) => request.slice(-8)));
  const takenIn = new Map<string, number>();
  // The first turn naming it took it: a later turn may carry it on as its first request.
  view.turns.forEach((each, index) => each.requests.forEach((request) => !takenIn.has(request.slice(-8)) && takenIn.set(request.slice(-8), index)));
  const lastTaken = new Map<string, number>();
  const readIds = new Set<string>();
  const running = view.turns.length - 1;
  const runs = view.turns[running]?.end == null;
  const inTaking = new Set<string>();
  let readIn = -1;
  view.entries.forEach((entry, position) => {
    if (!entry.parent && startsTurn(entry)) readIn += 1;
    const id = !entry.parent && entry.said?.by !== "user" && entry.event.type === "message" && entry.event.role === "user" ? queuedId(afterHandoff(entry.event.text)) : null;
    if (!id) return;
    readIds.add(id);
    const takingTurn = takenIn.get(id);
    // A turn begun while the taking one still ran (a CLI's queued input) is a read in it.
    const taking = takingTurn !== undefined && (readIn === takingTurn || (readIn > takingTurn && (view.turns[readIn]?.startedAtMs ?? 0) <= (view.turns[takingTurn]?.endedAtMs ?? Number.POSITIVE_INFINITY)));
    if (takingTurn === undefined) {
      if (runs && readIn === running) lastTaken.set(id, position);
      else lastTaken.delete(id);
    } else if (readIn < takingTurn || (taking && !inTaking.has(id))) {
      // Its last read before the turn that names it stands until that turn reads it: the live
      // turn's requests can name it a turn late.
      lastTaken.set(id, position);
      if (taking) inTaking.add(id);
    }
  });
  // The steers that name no queued message answer the turn's other requests, in order.
  const framed = (request: string) => readIds.has(request.slice(-8));
  view.entries.forEach((entry, position) => {
    if (entry.parent) return;
    if (startsTurn(entry)) turn += 1;
    let { event } = entry;
    // A successor's first message: the hand-off, then the user's words, if any.
    const handoff = entry.said?.by !== "user" && event.type === "message" && event.role === "user" && event.text.trimStart().startsWith(HANDOFF_MARK);
    if (handoff && event.type === "message") event = { ...event, text: afterHandoff(event.text) };
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
      if (startsTurn(entry) && !(maintenance === "turn" && brigadiers && entry.wake)) maintenance = null;
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
      !!asked && ((!!queued && (inRail.has(queued) || !tookIn.has(queued) || lastTaken.get(queued) !== position)) || (!said && onlyWithdrawn(asked.text)));
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
    // Also judged from the queue, or steered in, under its frame (goal 4).
    const peer = !!asked && FROM_SESSION.test(asked.text.replace(QUEUED, "").replace(STEERED, ""));
    const says =
      !!asked && (said && !peer ? ours : !theirs && BRIGADIER_SAYS.test(asked.text) && (!startsTurn(entry) || brigadiers || shownBefore));
    // A successor that carried on by itself: its first message is Brigadier's, the hand-off and
    // its word to carry on, no bubble; the successor's work shows.
    const own = !!asked && (carried.has(asked.itemId) || (!said && brigadiers && (handoff || asked.text.trim() === CARRY_ON)));
    // Brigadier's word that a command ended after its turn: its one plain line (D21).
    const wake = asked && says ? entry.wake : undefined;
    const woke = !!wake;
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
          goesOn(before, entry.requests ?? []));
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
      if (wake && !waitEnded(wake, view)) block?.taskNotices.push({ position, id: asked.itemId, text: wakeWords(wake), detail: asked.text, inWork: true });
      return;
    }
    if (startsTurn(entry) && event.type === "message") {
      // Before the turns are read again, the requests the engine noted it sent in the message;
      // a turn read before the engine noted them names none, so they join its own.
      const requests = [...new Set([...(view.turns[turn]?.requests ?? []), ...(entry.requests ?? [])])];
      const atMs = entry.atMs ?? 0;
      // A turn of a request an earlier turn carried (a background command's end, told after its
      // turn) starts with the CLI's message, not the user's: it keeps its own id. A queued message
      // the CLI began a turn with is its own request, which the turn names only once it joined:
      // the request before it may still lead the turn's list.
      const first = requests[0];
      const named = said?.by === "user" ? null : queuedId(event.text);
      // The request the engine noted it sent in the message says which it is, whenever it came.
      const id =
        entry.requests?.[0] ??
        (named
          ? (requests.find((request) => request.endsWith(named)) ?? `queued-${named}`)
          : first !== undefined && !blocks.some((earlier) => earlier.user?.kind === "message" && earlier.user.message.id === first)
            ? first
            : event.itemId);
      const shown = fromSession(userMessage(conversationId, entry, id, said?.by === "both" ? said.user : event.text, position + 1), theirs);
      // Sent while the block before still held: its bubble goes in that block, where it came,
      // and the turn's work goes on there.
      if (goesOn(block, entry.requests ?? [])) {
        join(block, position, entry.atMs);
        // One the block already shows (a switch's carry-on sends it again) shows once.
        const ids = [block.user?.kind === "message" ? block.user.message.id : null, ...block.steers.map((steer) => steer.message.id)];
        if (!ids.includes(shown.message.id)) block.steers.push({ ...shown, position });
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
          for (const request of entry.requests ?? []) if (!current.requestIds.includes(request)) current.requestIds.push(request);
          const id =
            entry.requests?.[0] ??
            (named
              ? (current.requestIds.find((request) => request.endsWith(named)) ?? `queued-${named}`)
              : (current.requestIds.slice(1).find((request) => !taken.has(request) && !framed(request)) ?? event.itemId));
          current.steers.push({ ...fromSession(userMessage(conversationId, entry, id, said?.by === "both" ? said.user : event.text, position + 1), theirs), position });
          return;
        }
        if (event.type === "message" && event.role !== "assistant") return;
        // A reply still streaming shows from its first word.
        const streaming = event.type === "messageDelta";
        const text = event.text;
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
          ...(streaming && { live: true }),
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
      case "taskNotice": {
        // An agent's end is its row's to say, as Codex's is none (D57, D96).
        const ended = entry.wake;
        if (ended && (ended.job === "agent" || waitEnded(ended, view))) return;
        current.taskNotices.push({ position, id: event.itemId, text: ended ? wakeWords(ended) : event.text, detail: event.text, inWork: true });
        return;
      }
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
        const tool = keptToolStep(entry, turnOf.get(current) ?? null, backgroundOf(view));
        // Its `brigadier send` to another session is the row of the message it sent (S30), its
        // `brigadier wait` on one the row of the reply (D138). Its other `wait`s and `start`s are
        // no rows (D47): the block's "Waiting for a worker" line and the worker's own row say them.
        // One call that sends and then waits (as the CLIs do) is the wait's row.
        const kind = sessionWait(tool, event, view) ?? sessionSent(tool, event, board)
          ?? (tool?.item === "command" && tool.detail !== null && brigadierUnshown(tool.detail) ? null : tool);
        if (kind) current.orchestratorSteps.push({ kind, position, ...(entry.atMs !== undefined && { atMs: entry.atMs }), requestId: current.key });
      }
    }
  });
  if (block) close(block);
  attachReplies(blocks.flatMap((each) => each.orchestratorSteps), view.replies ?? []);
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
    // A successor whose first message joined a block that still held: after that block.
    const into = seam.at === null ? undefined : joinedBy.get(seam.at);
    const next = seam.at === null ? blocks.length : into ? blocks.indexOf(into) + 1 : blocks.findIndex((each) => each.key === seam.at);
    // Never the note turn's own block, which the thread doesn't show.
    const before = next < 0 ? undefined : blocks.slice(0, next).findLast((each) => !internal.has(each));
    if (!before) continue;
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

/**
 * A turn of a message sent from here starts no later than its message did: the block shown while
 * it was on its way counted from then, so its clock goes on rather than starting again once the
 * CLI says it.
 */
function startBy(block: Block, atMs: number): void {
  const [first, ...rest] = block.worked;
  if (!first || atMs >= first.fromMs) return;
  block.startedAtMs = Math.min(block.startedAtMs, atMs);
  block.worked = [{ ...first, fromMs: atMs }, ...rest];
}

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
  // Another session's message the session took from its queue is kept as that session's (goal 4).
  const keptIds = stored.flatMap((message) => (message.role === "user" || message.role === "peer" ? [message.requestId ?? message.id] : []));
  const blocks = cliBlocks(view, conversationId, board, session, typed, keptIds);
  // The user's own messages as Brigadier keeps them, by their request: a turn that names one
  // shows it as it was sent, its pasted chips, mentions and attachments with it. The first under
  // a request is the user's; Brigadier's words kept after it under the same request are not.
  const own = new Map(stored.flatMap((message) => (message.role === "user" ? [[message.requestId ?? message.id, message] as const] : [])).toReversed());
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
    if (block.user?.kind === "message") {
      const kept = keptOf(block.user.message.id);
      block.user = { kind: "message", ...asSent(block.user.message, block.user.text) };
      if (kept) startBy(block, kept.createdAtMs);
    }
    block.steers = block.steers.map((steer) => (steer.message.role === "peer" ? steer : { ...steer, ...asSent(steer.message, steer.text) }));
    for (const said of [...(block.user?.kind === "message" ? [block.user] : []), ...block.steers]) drawn.add(said.message.requestId ?? said.message.id);
    // Another session's goes by its own id, the one it is kept under (goal 4).
    for (const said of [...(block.user?.kind === "peer" ? [block.user] : []), ...block.steers]) if (said.message.role === "peer") drawn.add(said.message.id);
  }
  // A turn's message the CLI said under a request whose message isn't stored here yet (its word
  // beat the daemon's answer to sending it) is the one still being sent: its bubble stays that
  // one, not a second bubble under the request. Only turns after the newest one of a message
  // held here: one before it is older than the messages loaded.
  const sent = new Map(view.entries.flatMap((entry) => (entry.event.type === "message" && entry.requests?.[0] ? [[entry.event.itemId, entry.requests[0]] as const] : [])));
  const said = new Set(view.entries.flatMap((entry) => entry.requests ?? []));
  const newest = blocks.findLastIndex((block) => block.user?.kind === "message" && own.has(block.user.message.id));
  const sending = [...pending];
  for (const block of blocks.slice(newest + 1)) {
    if (block.user?.kind !== "message" || !said.has(block.user.message.id)) continue;
    const entry = sending.shift();
    if (!entry) continue;
    block.user = { kind: "pending", pending: entry };
    startBy(block, entry.createdAtMs);
  }
  // A queued message joins the turn that read it after the CLI had it (the turn names it only
  // once it ends): it is drawn there, not on its way too.
  const carried = drawn;
  const heard = Math.max(
    0,
    ...view.entries.flatMap((entry) => (entry.event.type === "message" && entry.event.role === "user" && !entry.parent ? [entry.atMs ?? 0] : [])),
  );
  // A queued message keeps its id when it is sent; until the rail lets it go it shows there only.
  const queued = new Set((board.queueItems ?? []).map((item) => item.id));
  for (const message of stored) {
    const peer = message.role === "peer";
    if ((message.role !== "user" && !peer) || message.createdAtMs <= heard || carried.has(message.requestId ?? message.id) || queued.has(message.id)) continue;
    const text = fullText[message.id] ?? message.text;
    // One the user steered into a request a block shows is that block's (D25), after its items,
    // until the CLI says where it read it: never a block of its own (seen live at a switch).
    const into = (board.requests?.[message.requestId ?? message.id] as { steeredInto?: string | null } | undefined)?.steeredInto;
    const open = into ? blocks.findLast((block) => block.requestIds.includes(into)) : undefined;
    if (open && !peer) {
      open.steers.push({ message, text, position: view.entries.length });
      continue;
    }
    // Another session's, taken from the queue, is on its way as that session's (goal 4).
    const block = onItsWay(message.id, peer ? { kind: "peer", message, text: peerWords(text), earlier: false } : { kind: "message", message, text }, message.createdAtMs, run);
    // Its new worktree's setup line, while its turn starts or after its start failed.
    const setup = (board.startSteps ?? []).filter((record) => record.requestId === (message.requestId ?? message.id));
    if (setup.length > 0) block.orchestratorSteps.push({ kind: { type: "setup", steps: setup }, position: 0 });
    blocks.push(block);
  }
  for (const entry of sending) blocks.push(onItsWay(entry.localId, { kind: "pending", pending: entry }, entry.createdAtMs, { state: "running", error: null }));

  const nodes: ThreadNode[] = [];
  let parent: string | null = null;
  // A follow-up woken to be judged, judged wait, then sent on its own turn leads two turns: the
  // one that shows it keeps its request's id (a repeated id makes assistant-ui throw and blanks the app).
  const showing = new Set(blocks.flatMap((block) => (block.user?.kind === "message" ? [shownIdOf(block.user.message.id)] : [])));
  for (const block of blocks) {
    // A message sent from here keeps the id it showed under while pending (`shownIdOf`), so
    // its bubble isn't mounted afresh when it is stored and its turn is placed once it shows.
    let shown = block.user && (block.user.kind === "pending" ? block.user.pending.localId : shownIdOf(block.user.message.id));
    // A repeated id makes assistant-ui throw, which blanked the whole window (seen live at a switch).
    if (shown && nodes.some((node) => node.id === shown)) shown = `${shown}:${block.key}`;
    if (shown) {
      nodes.push({ id: shown, parentId: parent, kind: "user", block, head: null });
      parent = shown;
    }
    if (!blockShows(block)) continue;
    // Its turn by its message too, else by the request the CLI's message it began with carries
    // (one not shown yet): the same from sending it to the CLI saying it to its turn read.
    const lead = sent.get(block.key);
    const wanted = `request:${shown || (lead && !showing.has(shownIdOf(lead)) ? shownIdOf(lead) : block.key)}`;
    // Two turns leading with it before either shows it: the later goes by its own key.
    const id = nodes.some((node) => node.id === wanted) ? `request:${block.key}` : wanted;
    nodes.push({ id, parentId: parent, kind: "block", block, head: null });
    parent = id;
  }
  return { nodes, headId: parent };
}
