import { isPlumbing, toolName, type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import { type Block, type BlockState, blockShows, isLive, isUpdate, type ThreadNode, type ThreadTree } from "@/app/conversation/blocks";
import { INTERRUPTED } from "@/app/conversation/turnError";
import type {
  FileChange,
  FileChangeKind,
  FileEntry,
  FileStat,
  Message,
  ThreadItemKind,
  ThreadTurn,
  ThreadView,
  UserRequest,
} from "@/ipc/generated";
import { sessionImageRef } from "@/state/cliThread";
import type { PendingMessage } from "@/state/store";

/**
 * The thread drawn from the CLI's own session file (S9): the engine's `ThreadView` as the same
 * blocks Brigadier's own records give (`buildBlocks`), one per turn, so the thread looks exactly
 * as it does. A turn is the user's message up to the CLI's end of it; its replies, reasoning and
 * calls are the CLI's items in the CLI's order, and its time runs by the CLI's timestamps.
 *
 * What the session file doesn't hold (workers, cards, messages between sessions, waits) is not
 * drawn here: it comes back from the CLI in later steps.
 */

/** A tool's own name, without its MCP server's prefix (`mcp__brigadier__run` → `run`). */
const bare = toolName;

/** Shell tools: their call is a command (`digest::is_shell_tool`, `own_run`). */
const SHELL_TOOLS = new Set(["Bash", "run", "run_unsandboxed"]);

/** Calls whose trace is a card or a row of their own, which the session file doesn't hold. */
const WITHOUT_ROW = new Set(["propose_plan", "wait_for_replies"]);

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
 * for what is no row: Brigadier's plumbing, whose trace is elsewhere.
 */
export function toolStep(entry: FileEntry, stopped: boolean): ToolStep | null {
  const { event } = entry;
  const base = { throughPosition: 0 };
  switch (event.type) {
    case "toolCall": {
      const name = bare(event.name);
      if (isPlumbing(name) || WITHOUT_ROW.has(name)) return null;
      const endedAtMs = endedAt(entry, event.status);
      const exit = SHELL_TOOLS.has(name) && endedAtMs !== undefined && event.output !== null ? exitOf(event.output) : undefined;
      return {
        type: "tool",
        itemId: event.itemId,
        name,
        detail: stepDetail(event.input),
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

/** The user's message of a turn (or a steer) as the thread's message record. */
function userMessage(conversationId: string, entry: FileEntry, itemId: string, text: string, seq: number): Message {
  return {
    id: itemId,
    conversationId,
    seq,
    role: "user",
    text,
    blob: null,
    createdAtMs: entry.atMs ?? 0,
    attachments: (entry.images ?? []).map(sessionImageRef),
    mentions: [],
    model: null,
    requestId: itemId,
    parentId: null,
  };
}

function emptyBlock(key: string, startedAtMs: number): Block {
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
    requestIds: [key],
    state: "working",
    error: null,
    startedAtMs,
    endedAtMs: null,
    worked: [{ fromMs: startedAtMs, toMs: null }],
    waited: [],
    quotaWait: false,
    cardWait: false,
    waitingFor: [],
  };
}

/** Whether an entry starts a turn: the user's own message in the session's thread. */
function startsTurn(entry: FileEntry): boolean {
  return entry.event.type === "message" && entry.event.role === "user" && !entry.parent && !entry.steer;
}

/** A turn's state, start and end set on its block, from how the CLI ended it. */
function settle(block: Block, turn: ThreadTurn | undefined): void {
  const { state, error } = turnState(turn ?? { end: null, error: null });
  const startedAtMs = turn?.startedAtMs ?? block.startedAtMs;
  const endedAtMs = turn?.end ? (turn.endedAtMs ?? startedAtMs) : null;
  Object.assign(block, {
    state,
    error,
    startedAtMs,
    endedAtMs,
    worked: [{ fromMs: startedAtMs, toMs: endedAtMs }],
  } satisfies Partial<Block>);
  // A turn over has no reasoning still coming.
  if (endedAtMs !== null) block.thinking = block.thinking.map((segment) => (segment.complete ? segment : { ...segment, complete: true }));
}

/**
 * A thread's turns as blocks, oldest first. A subagent's own items (`parent`) are not drawn:
 * the thread shows its Agent call's row, as it does for a Claude subagent's work.
 */
export function cliBlocks(view: ThreadView, conversationId: string): Block[] {
  const blocks: Block[] = [];
  let block: Block | null = null;
  view.entries.forEach((entry, position) => {
    if (entry.parent) return;
    const { event } = entry;
    if (startsTurn(entry) && event.type === "message") {
      if (block) settle(block, view.turns[blocks.length - 1]);
      block = emptyBlock(event.itemId, entry.atMs ?? 0);
      block.user = { kind: "message", message: userMessage(conversationId, entry, event.itemId, event.text, position + 1), text: event.text };
      blocks.push(block);
      return;
    }
    // Anything before the first user message belongs to no turn.
    if (!block) return;
    const current: Block = block;
    const ended = view.turns[blocks.length - 1]?.end ?? null;
    switch (event.type) {
      case "message":
      case "messageDelta": {
        if (event.type === "message" && event.role === "user") {
          const message = userMessage(conversationId, entry, event.itemId, event.text, position + 1);
          current.steers.push({ message, text: event.text, position });
          return;
        }
        if (event.type === "message" && event.role !== "assistant") return;
        current.texts.push({
          messageId: event.itemId,
          position,
          text: event.text,
          model: null,
          update: isUpdate(event.text),
          ...(event.type === "messageDelta" && event.phase === "answer" && entry.atMs !== undefined && { answerAtMs: entry.atMs }),
          ...(event.type === "messageDelta" && event.phase === "commentary" && { commentary: true }),
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
      case "compactionEnded": {
        const atMs = entry.atMs ?? current.startedAtMs;
        current.compactions.push({
          id: `compaction:${position}`,
          inTurn: true,
          automatic: event.automatic,
          state: event.error === null ? "done" : "failed",
          error: event.error,
          position,
          startedAtMs: atMs,
          endedAtMs: atMs,
        });
        return;
      }
      default: {
        const kind = toolStep(entry, ended === "stopped");
        if (kind) current.orchestratorSteps.push({ kind, position, ...(entry.atMs !== undefined && { atMs: entry.atMs }), requestId: current.key });
      }
    }
  });
  if (block) settle(block, view.turns[blocks.length - 1]);
  return blocks;
}

/** A block of a message sent but not in the CLI's file yet: the bubble, and a block that works on it. */
function onItsWay(key: string, user: Block["user"], startedAtMs: number, request?: UserRequest): Block {
  const block = { ...emptyBlock(key, startedAtMs), user };
  if (!request || isLive(request.state.type)) return block;
  // It never reached the CLI: its request says how it ended.
  return {
    ...block,
    state: request.state.type,
    error: request.state.type === "failed" ? request.state.error : null,
    endedAtMs: request.endedAtMs,
    worked: [{ fromMs: startedAtMs, toMs: request.endedAtMs }],
  };
}

/**
 * The thread as assistant-ui's message tree: the CLI's turns, one after another, then the
 * messages still on their way: sent but not stored (`pending`), and stored but not yet in the
 * CLI's file (a user message newer than the newest it has, whose request runs or failed).
 */
export function cliThreadTree(
  view: ThreadView,
  conversationId: string,
  stored: readonly Message[],
  fullText: Readonly<Record<string, string>>,
  pending: readonly PendingMessage[],
  requests: Readonly<Record<string, UserRequest>>,
): ThreadTree {
  const blocks = cliBlocks(view, conversationId);
  const heard = Math.max(
    0,
    ...view.entries.flatMap((entry) => (entry.event.type === "message" && entry.event.role === "user" && !entry.parent ? [entry.atMs ?? 0] : [])),
  );
  for (const message of stored) {
    if (message.role !== "user" || message.createdAtMs <= heard) continue;
    const request = requests[message.requestId ?? message.id];
    if (request && request.state.type !== "failed" && !isLive(request.state.type)) continue;
    const text = fullText[message.id] ?? message.text;
    blocks.push(onItsWay(message.id, { kind: "message", message, text }, message.createdAtMs, request));
  }
  for (const entry of pending) blocks.push(onItsWay(entry.localId, { kind: "pending", pending: entry }, entry.createdAtMs));

  const nodes: ThreadNode[] = [];
  let parent: string | null = null;
  for (const block of blocks) {
    if (block.user) {
      const id = block.user.kind === "pending" ? block.user.pending.localId : block.key;
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
