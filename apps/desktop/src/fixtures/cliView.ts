/**
 * Fixtures only: a conversation's thread as its CLI would keep it (`ThreadView`), from what a
 * fixture sets up: its stored messages (`useApp` threads), its requests and tool steps
 * (`useBoard`), and the live turn's streaming answer and thinking. Importing this keeps every
 * such conversation's view in step with the fixture as it drives the stores, the way the
 * engine's `threadView` and `threadNews` keep the app's.
 */
import type { FileChangeKind, FileEntry, Message, OrchestratorStep, ProviderEvent, ThreadTurn, ThreadView, TurnEnd, UserRequest } from "@/ipc/generated";
import { applyActivityNews } from "@/state/activity";
import { type Board, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import { rowOf } from "@/fixtures/workerRows";
import { useApp } from "@/state/store";

type ToolKind = Extract<OrchestratorStep["kind"], { type: "tool" }>;

/** A tool step as the CLI's own item. */
function toolEvent(step: ToolKind): ProviderEvent {
  const status = step.status;
  if (step.name === "shell" || step.item === "command") {
    return {
      type: "command",
      itemId: step.itemId,
      command: step.detail ?? "",
      cwd: "",
      status,
      exitCode: step.exit ?? null,
      output: step.fullOutput ?? null,
      durationMs: null,
    } as ProviderEvent;
  }
  if (step.item === "files" && step.files) {
    return {
      type: "fileChanges",
      itemId: step.itemId,
      changes: step.files.map((file) => ({ path: file.path, kind: (file.change ?? "update") as FileChangeKind, added: file.insertions, removed: file.deletions })),
      status,
    } as ProviderEvent;
  }
  if (step.name === "imagegen") return { type: "image", itemId: step.itemId, prompt: step.detail, status } as unknown as ProviderEvent;
  const key = step.item === "web" ? "query" : step.item === "image" || step.item === "files" ? "file_path" : "query";
  return {
    type: "toolCall",
    itemId: step.itemId,
    name: step.name,
    input: step.detail === null ? null : JSON.stringify({ [key]: step.detail }),
    status,
    output: step.fullOutput ?? null,
  } as ProviderEvent;
}

/** How a turn ended, by its request; a turn without one (a fixture's stored messages only) is over (at its last item, set below). */
function endOf(request: UserRequest | undefined): Pick<ThreadTurn, "end" | "error" | "endedAtMs"> {
  if (!request) return { end: "done", error: null, endedAtMs: null };
  const state = request.state;
  const end: TurnEnd | null = state.type === "working" || state.type === "waiting" ? null : (state.type as TurnEnd);
  return { end, error: state.type === "failed" ? state.error : null, endedAtMs: end === null ? null : (request.endedAtMs ?? request.startedAtMs ?? null) };
}

/**
 * How far the CLI's thread lags behind the fixture's stores, as over a busy connection: the
 * messages its CLI hasn't said yet (`unsaid`), and those it said whose turn the view hasn't read
 * again (`unread`). With `ownIds`, a message sent in the fixture (`sent-…`) is the CLI's item
 * under an id of its own, as a real CLI's is. See `cliSays` and `cliTurnsRead`.
 */
export const cliLag = { unsaid: new Set<string>(), unread: new Set<string>(), ownIds: false };

/** The CLI's id for stored message `id`. */
function cliIdOf(id: string): string {
  return cliLag.ownIds && id.startsWith("sent-") ? `cli-${id}` : id;
}

/** A conversation's view from its fixture's stores. */
export function fixtureView(messages: readonly Message[], board: Board | null): ThreadView {
  const requests = board?.requests ?? {};
  const placed: { position: number; atMs: number; entry: FileEntry }[] = [];
  for (const message of messages) {
    if ((message.role !== "user" && message.role !== "assistant") || cliLag.unsaid.has(message.id)) continue;
    const request = message.requestId ? requests[message.requestId] : undefined;
    const steer = message.role === "user" && !!request?.steeredInto;
    // A final answer stored: the CLI's item began when the answer started, and is the answer.
    const answer = message.role === "assistant" && message.answerAtMs !== undefined;
    const atMs = answer ? message.answerAtMs! : message.createdAtMs;
    placed.push({
      position: message.seq,
      atMs,
      entry: {
        event: { type: "message", itemId: cliIdOf(message.id), role: message.role, text: message.text },
        atMs,
        ...(steer && { steer: true }),
        ...(answer && { phase: "answer" as const }),
      },
    });
  }
  for (const step of board?.orchestratorSteps ?? []) {
    if (step.kind.type !== "tool") continue;
    placed.push({
      position: step.position,
      atMs: step.atMs,
      entry: { event: toolEvent(step.kind), atMs: step.atMs, ...(step.kind.endedAtMs !== undefined && { updatedAtMs: step.kind.endedAtMs }) },
    });
  }
  for (const segment of board?.thinking ?? []) {
    placed.push({
      position: segment.position,
      atMs: segment.startedAtMs,
      entry: { event: { type: segment.complete ? "reasoning" : "reasoningDelta", itemId: segment.itemId, text: segment.text }, atMs: segment.startedAtMs, updatedAtMs: segment.updatedAtMs },
    });
  }
  const streaming = board?.streaming;
  if (streaming && !messages.some((message) => message.id === streaming.messageId)) {
    placed.push({
      position: Number.MAX_SAFE_INTEGER,
      atMs: streaming.startedAtMs,
      entry: { event: { type: "messageDelta", itemId: streaming.messageId, text: streaming.text, ...(streaming.phase && { phase: streaming.phase }) }, atMs: streaming.startedAtMs },
    });
  }
  placed.sort((a, b) => a.position - b.position || a.atMs - b.atMs);
  const entries = placed.map((each) => each.entry);

  const turns: ThreadTurn[] = [];
  // Each turn's last item's time, for a turn without a request.
  const lastAt: (number | null)[] = [];
  for (const { entry, atMs } of placed) {
    const { event } = entry;
    if (event.type !== "message" || event.role !== "user") {
      if (lastAt.length > 0) lastAt[lastAt.length - 1] = atMs;
      continue;
    }
    const message = messages.find((each) => cliIdOf(each.id) === event.itemId);
    const id = message?.requestId ?? event.itemId;
    if (entry.steer) {
      turns.at(-1)?.requests.push(id);
      continue;
    }
    const request = requests[id];
    if (message && cliLag.unread.has(message.id)) continue;
    turns.push({ userMessage: event.itemId, startedAtMs: request?.startedAtMs ?? entry.atMs ?? null, ...endOf(request), requests: [id] });
    lastAt.push(entry.atMs ?? null);
  }
  turns.forEach((turn, index) => {
    if (!requests[turn.requests[0] ?? ""]) turn.endedAtMs = lastAt[index] ?? turn.startedAtMs;
  });
  // A turn steered into reads as the request it ended with.
  for (const turn of turns) {
    const last = requests[turn.requests.at(-1) ?? ""];
    if (turn.requests.length > 1 && last) Object.assign(turn, endOf(last));
  }
  return { entries, turns };
}

/** How each conversation's newest turn stood when last said: open, or how it ended. */
const told = new Map<string, TurnEnd | "open" | "none">();

/** Each thread's view as last made, with what it was made from: one that didn't change keeps its view, so its rows don't draw again. */
const made = new Map<string, { items: readonly Message[]; board: Board | null; view: ThreadView }>();

/** Every conversation the fixture holds a thread of, its view kept in step, and its turn's opening and end said as the stream says them. */
function sync(): void {
  const { threads } = useApp.getState();
  const { board } = useBoard.getState();
  const views: Record<string, ThreadView> = {};
  // A thread still loading with nothing held yet has no view: it reads as loading.
  const unread = new Set<string>();
  for (const [id, thread] of Object.entries(threads)) {
    if (thread.loading && thread.items.length === 0) {
      unread.add(id);
      made.delete(id);
      continue;
    }
    const own = board?.conversationId === id ? board : null;
    const last = made.get(id);
    if (last && last.items === thread.items && last.board === own) continue;
    // Its workers as their rows (S14), from the fixture's tasks; what else the view holds stays.
    const held = useCliThread.getState().views[id];
    const workers = own ? Object.values(own.tasks).toSorted((a, b) => b.createdAtMs - a.createdAtMs).map((task) => rowOf(task, {}, own.diffs[task.id] ?? null)) : (held?.workers ?? []);
    views[id] = { ...held, ...fixtureView(thread.items, own), workers };
    made.set(id, { items: thread.items, board: own, view: views[id] });
  }
  const { views: held } = useCliThread.getState();
  if (Object.keys(views).length > 0 || [...unread].some((id) => held[id])) {
    useCliThread.setState((state) => ({
      views: { ...Object.fromEntries(Object.entries(state.views).filter(([id]) => !unread.has(id))), ...views },
    }));
  }
  for (const [id, view] of Object.entries(views)) {
    const last = view.turns.at(-1);
    const now = !last ? "none" : (last.end ?? "open");
    if (told.get(id) === now) continue;
    told.set(id, now);
    if (now === "open") applyActivityNews({ conversationId: id, entries: [], opened: true });
    else if (now !== "none") applyActivityNews({ conversationId: id, entries: [], ended: now });
  }
}

/** The CLI says stored message `id` (its turn not read yet): the views show it. */
export function cliSays(id: string): void {
  cliLag.unsaid.delete(id);
  cliLag.unread.add(id);
  made.clear();
  sync();
}

/** The views read their turns again: every turn said shows. */
export function cliTurnsRead(): void {
  cliLag.unread.clear();
  made.clear();
  sync();
}

sync();
useApp.subscribe((state, previous) => {
  if (state.threads !== previous.threads) sync();
});
useBoard.subscribe((state, previous) => {
  if (state.board !== previous.board) sync();
});
