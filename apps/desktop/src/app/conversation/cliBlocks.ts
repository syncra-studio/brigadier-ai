import { isPlumbing, toolName, type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import {
  type Block,
  type BoardDigest,
  type BlockState,
  blockShows,
  boardItems,
  isUpdate,
  type ThreadNode,
  type ThreadTree,
} from "@/app/conversation/blocks";
import { INTERRUPTED } from "@/app/conversation/turnError";
import type {
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
      if (isPlumbing(name)) return null;
      const endedAtMs = endedAt(entry, event.status);
      const exit = SHELL_TOOLS.has(name) && endedAtMs !== undefined && event.output !== null ? exitOf(event.output) : undefined;
      return {
        type: "tool",
        itemId: event.itemId,
        // As the CLI named it: an MCP call's server is part of its words ("Used Linear").
        name: event.name,
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

/** Each entry's row as last made: an item the stream left alone keeps its entry (`mergeEntries`). */
const madeSteps = new WeakMap<FileEntry, { stopped: boolean; step: ToolStep | null }>();

/** `toolStep`, made once per entry: the thread is made again with each streamed word. */
function keptToolStep(entry: FileEntry, stopped: boolean): ToolStep | null {
  const made = madeSteps.get(entry);
  if (made?.stopped === stopped) return made.step;
  const step = toolStep(entry, stopped);
  madeSteps.set(entry, { stopped, step });
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
function fromSession(message: Message): { message: Message; text: string } {
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
        block.compactions.push({ ...item.compaction, position });
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
 * (`board`). A subagent's own items (`parent`) are not drawn: the thread shows its Agent call's
 * row, as it does for a Claude subagent's work. `session`: the thread is a session's, where
 * Brigadier compacts a long one itself.
 */
export function cliBlocks(view: ThreadView, conversationId: string, board?: BoardDigest, session = false): Block[] {
  const blocks: Block[] = [];
  // Each block's CLI items, by position and time.
  const times: [number, number][][] = [];
  let block: Block | null = null;
  const seams = view.seams ?? [];
  const nudges = new Set(seams.flatMap((seam) => (seam.nudge ? [seam.nudge] : [])));
  // A successor that started by itself: its first message is Brigadier's word to carry on.
  const carried = new Set(seams.flatMap((seam) => (seam.carried && seam.at ? [seam.at] : [])));
  // A queued message shows once (run2-D44): in the rail while the CLI has it there, else
  // where the CLI last took it (a message that waited for its own turn was read before).
  const inRail = new Set((board?.queueItems ?? []).flatMap((item) => (item.delivered ? [item.id.slice(-8)] : [])));
  const lastTaken = new Map<string, number>();
  view.entries.forEach((entry, position) => {
    const id = !entry.parent && entry.event.type === "message" && entry.event.role === "user" ? queuedId(entry.event.text) : null;
    if (id) lastTaken.set(id, position);
  });
  // The steers that name no queued message answer the turn's other requests, in order.
  const framed = (request: string) => lastTaken.has(request.slice(-8));
  view.entries.forEach((entry, position) => {
    if (entry.parent) return;
    const { event } = entry;
    // Brigadier's one message at a seam is its line in the turn, never a bubble of the user's;
    // sent as a turn of its own, that turn has no user message, as a successor's that carried on.
    const asked = event.type === "message" && event.role === "user" ? event : null;
    const own = !!asked && carried.has(asked.itemId);
    // Brigadier's message of what the workers did is its quiet lines in the turn, the same way.
    const news = asked && workerNotices(asked.text);
    const queued = asked && queuedId(asked.text);
    const unshown =
      !!asked && ((!!queued && (inRail.has(queued) || lastTaken.get(queued) !== position)) || onlyWithdrawn(asked.text));
    // A long session's `/compact` is Brigadier's (run2-D5): the thread shows the compaction's
    // divider, never the command or what its summary must keep.
    // Its checkpoint request before Codex compacts is Brigadier's too (run2-D9).
    // Brigadier's own turn carries no request of the user's: what the user sent shows as ever.
    const brigadiers = startsTurn(entry) && view.turns[blocks.length]?.requests.length === 0;
    const compacting =
      !!asked && session && brigadiers && (/^\/compact(\s|$)/.test(asked.text) || asked.text.startsWith(CHECKPOINT_MARK));
    if (asked && (own || compacting || news || unshown || nudges.has(asked.itemId))) {
      if (startsTurn(entry)) {
        if (block) settle(block, view.turns[blocks.length - 1]);
        block = emptyBlock(asked.itemId, entry.atMs ?? 0, view.turns[blocks.length]?.requests ?? []);
        blocks.push(block);
        times.push([[position, entry.atMs ?? 0]]);
      }
      if (entry.atMs !== undefined && !startsTurn(entry)) times.at(-1)?.push([position, entry.atMs]);
      if (news) news.forEach((text, nth) => block?.taskNotices.push({ position: position + nth / 1000, text }));
      else if (!own && !compacting && !unshown) block?.nudges.push(position);
      return;
    }
    if (startsTurn(entry) && event.type === "message") {
      if (block) settle(block, view.turns[blocks.length - 1]);
      const requests = view.turns[blocks.length]?.requests ?? [];
      const atMs = entry.atMs ?? 0;
      block = emptyBlock(event.itemId, atMs, requests);
      // A turn of a request an earlier turn carried (a background command's end, told after its
      // turn) starts with the CLI's message, not the user's: it keeps its own id.
      const first = requests[0];
      const id = first !== undefined && !blocks.some((earlier) => earlier.user?.kind === "message" && earlier.user.message.id === first) ? first : event.itemId;
      const said = fromSession(userMessage(conversationId, entry, id, event.text, position + 1));
      block.user = said.message.role === "peer" ? { kind: "peer", ...said, earlier: false } : { kind: "message", ...said };
      blocks.push(block);
      times.push([[position, atMs]]);
      return;
    }
    // Anything before the first user message belongs to no turn.
    if (!block) return;
    const current: Block = block;
    if (entry.atMs !== undefined) times.at(-1)?.push([position, entry.atMs]);
    const ended = view.turns[blocks.length - 1]?.end ?? null;
    switch (event.type) {
      case "message":
      case "messageDelta": {
        if (event.type === "message" && event.role === "user") {
          // A steer answers its own request, the next the turn names; a queued message's is
          // its own id (the request it joined as).
          const named = queuedId(event.text);
          const taken = new Set(current.steers.map((steer) => steer.message.id));
          const id = named
            ? (current.requestIds.find((request) => request.endsWith(named)) ?? `queued-${named}`)
            : (current.requestIds.slice(1).find((request) => !taken.has(request) && !framed(request)) ?? event.itemId);
          current.steers.push({ ...fromSession(userMessage(conversationId, entry, id, event.text, position + 1)), position });
          return;
        }
        if (event.type === "message" && event.role !== "assistant") return;
        // What the reply is, as the stream said it, else as the file keeps it (Codex's `phase`).
        const phase = (event.type === "messageDelta" ? event.phase : undefined) ?? entry.phase;
        current.texts.push({
          messageId: event.itemId,
          position,
          text: event.text,
          model: null,
          update: isUpdate(event.text),
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
      case "taskNotice":
        current.taskNotices.push({ position, text: event.text });
        return;
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
        const kind = keptToolStep(entry, ended === "stopped");
        if (kind) current.orchestratorSteps.push({ kind, position, ...(entry.atMs !== undefined && { atMs: entry.atMs }), requestId: current.key });
      }
    }
  });
  if (block) settle(block, view.turns[blocks.length - 1]);
  if (board) place(blocks, times, board);
  // Each seam's divider after the turn before its successor's first message, or after the last
  // turn while the successor is still to come.
  for (const seam of seams) {
    if (seam.open) continue;
    const next = seam.at === null ? blocks.length : blocks.findIndex((each) => each.key === seam.at);
    const before = blocks[next < 0 ? -1 : next - 1];
    if (next < 0 || !before) continue;
    before.seams.push({ n: seam.n, note: seam.note, start: seam.start, death: seam.death, movedTo: seam.movedTo });
  }
  return blocks;
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
 */
export function cliThreadTree(
  view: ThreadView,
  conversationId: string,
  stored: readonly Message[],
  fullText: Readonly<Record<string, string>>,
  pending: readonly PendingMessage[],
  board: BoardDigest,
  run: RunNow,
  session = false,
): ThreadTree {
  const blocks = cliBlocks(view, conversationId, board, session);
  // The user's own messages as Brigadier keeps them, by their request: a turn that names one
  // shows it as it was sent, its pasted chips, mentions and attachments with it.
  const own = new Map(stored.flatMap((message) => (message.role === "user" ? [[message.requestId ?? message.id, message] as const] : [])));
  const asSent = (message: Message, text: string): { message: Message; text: string } => {
    const kept = own.get(message.id);
    return kept ? { message: kept, text: fullText[kept.id] ?? kept.text } : { message, text };
  };
  for (const block of blocks) {
    if (block.user?.kind === "message") block.user = { kind: "message", ...asSent(block.user.message, block.user.text) };
    block.steers = block.steers.map((steer) => (steer.message.role === "peer" ? steer : { ...steer, ...asSent(steer.message, steer.text) }));
  }
  const carried = new Set(view.turns.flatMap((turn) => turn.requests));
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
