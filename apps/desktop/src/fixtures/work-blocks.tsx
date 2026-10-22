/**
 * A standalone Vite entry, excluded from the app entry and production build: one turn's work
 * block in the app's own conversation view, driven through the moments of a turn by changing its
 * board as the daemon's events would, each measured once the page holds still.
 *
 * - `drive=turn`: a Chat's turn reads a file and runs the tests; then it thinks, a question waits
 *   on the user, a command runs, a line between steps streams, the final answer streams, is
 *   stored, it is done, and the board is read again from the daemon.
 * - `drive=steer`: a message steered in while a command runs.
 * - `drive=queue` (`kind=session` for a session): a queued message's ⋯ menu, and turning
 *   queueing off from it.
 * - `drive=background` (with `kind=session`): the thread's check goes to the background and it
 *   says so; the request works on with a live line and a context-card row (Stop) until the
 *   result's answer.
 * - `drive=timer`: the turn waits on its question card, then ends; its header shows the work and the
 *   wait for the user apart (BUILD-PLAN A9).
 * - `drive=joined` (with `kind=session`): a follow-up joined the turn as it ended; both answers stay
 *   in view.
 * - `drive=filelink` (with `kind=session`): a finished answer's file chip holds still while a later
 *   turn works.
 * - `drive=answerfile` (`kind=session` for a session): a finished answer names its worker's file once,
 *   inline (Q84): nothing lists files under it, no Save sits by the link, and the link opens the file
 *   in the reader, with its Save. `stop=answer|reader` holds the page for a screenshot.
 * - `drive=update` (`kind=session` for a session): an ending that opens with "Update:" and has a
 *   `### Details` section is the answer, its Details folded; the opening line folds into the work.
 *
 * The result goes to `<pre id="work-blocks-result">`. `stop=running|answering|done|steer|menu` holds the
 * page at that moment instead, for a screenshot.
 */
import "@/fixtures/cliView";
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type {
  BackgroundRun,
  Conversation,
  ConversationView as View,
  Message,
  OrchestratorStep,
  Plan,
  Question,
  QueuedMessage,
  Request,
  Settings,
  Task,
  ThinkingSegment,
  UserRequest,
} from "@/ipc/generated";
import { answerHtml } from "@/app/conversation/AnswerActions";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import { fixtureClick } from "@/fixtures/input";
import { loadConversation, openConversation } from "@/state/actions";
import { type Board, type ShownApproval, useBoard } from "@/state/board";
import { closeTab, sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const drive = query.get("drive") ?? "turn";
const ID = "work-blocks";
const SIDE = "work-blocks-side";
const session = query.get("kind") === "session";
const begun = Date.now() - 30_000;

function conversation(id: string, title: string, kind: "chat" | "session"): Conversation {
  return {
    id, kind, projectId: null, title, pinnedAtMs: null, createdAtMs: begun, updatedAtMs: begun, lifecycle: "active",
    forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
    setup: kind === "chat"
      ? { type: "chat", model: { provider: "codex", model: null, effort: null, fast: false } }
      : {
          type: "session", repo: "/tmp/work-blocks-fixture", environment: { type: "local" }, permission: "full", planMode: false,
          workersSeeUncommitted: null, orchestrator: { provider: "codex", model: null, effort: null },
        },
  } as unknown as Conversation;
}

const conversations: Record<string, Conversation> = {
  [ID]: conversation(ID, "Fix the rounding", session ? "session" : "chat"),
};

const user = (id: string, seq: number, text: string, atMs: number): Message => ({
  id, conversationId: ID, seq, role: "user", text, blob: null, createdAtMs: atMs, attachments: [], mentions: [], model: null,
  requestId: id, parentId: null,
});
const asked = (id: string, atMs: number, more: Partial<UserRequest> = {}): UserRequest => ({
  id, conversationId: ID, preview: "", state: { type: "working" }, startedAtMs: atMs, endedAtMs: null, steeredInto: null,
  steeredAfter: null, undo: null, worked: [{ fromMs: atMs, toMs: null }], quotaWait: false, ...more,
} as UserRequest);
const tool = (itemId: string, position: number, name: string, detail: string, status: "inProgress" | "completed"): OrchestratorStep => ({
  requestId: "r1", position, atMs: begun + position * 1000,
  kind: { type: "tool", itemId, name, status, detail, throughPosition: position },
} as OrchestratorStep);

const LONG_MESSAGE = [
  ...Array.from({ length: 15 }, (_, index) => `Line ${index + 1} of the request.`),
  "",
  "```ts",
  ...Array.from({ length: 12 }, (_, index) => `const v${index} = ${index};`),
  "```",
  "",
  ...Array.from({ length: 10 }, (_, index) => `After ${index + 1}.`),
].join("\n");

/** What the daemon holds now: the view a load reads. */
const daemon: { messages: Message[]; requests: Record<string, UserRequest>; steps: OrchestratorStep[]; run: "running" | "idle" } = {
  // `drive=clip`: a long message whose code block crosses its 20-line cut.
  messages: [user("r1", 1, drive === "clip" ? LONG_MESSAGE : "Fix the rounding in math.js", begun)],
  requests: { r1: asked("r1", begun) },
  // `drive=early`: the turn has done nothing yet.
  steps: drive === "early" ? [] : [tool("read", 1.1, "Read", "/repo/src/math.js", "completed"), tool("test", 1.2, "shell", "npm test", "completed")],
  run: "running",
};
const queued: QueuedMessage = {
  id: "q1", text: "Also check the currency formatting", attachments: [], mentions: [], queuedAtMs: begun, editedAtMs: null, deciding: false,
};

function view(): View {
  return {
    conversation: conversations[ID]!, context: null,
    messages: { messages: daemon.messages, hasMore: false },
    tasks: [], approvals: [], questions: [], plans: [], requests: Object.values(daemon.requests), workerSteps: [],
    orchestratorSteps: daemon.steps, machineSteps: [], thinking: [], compactions: [], decisions: [], waiting: [],
    queue: { items: drive === "queue" ? [queued] : [], paused: false },
    run: daemon.run, runRequest: daemon.run === "running" ? "r1" : null,
    head: daemon.messages.at(-1)?.id ?? null, ratings: {}, streaming: null,
    notices: [], memories: [], overnight: [], previews: [],
  } as unknown as View;
}

let settings = { followUp: "queue" } as Settings;
let failLoads = drive === "loadfail";
const calls: string[] = [];
/** `drive=answerfile`: the worker's file its answer names, as Brigadier stored it. */
const NOTES = { id: "blob-notes", title: "notes.md", kind: "note", mime: "text/markdown", bytes: 120, fileName: "notes.md" } as const;
const NOTES_TEXT = "# Rounding cases\n\n- 2.345 rounds to **2.34** (half-even)\n- 2.355 rounds to **2.36**\n- Totals are rounded once, after tax.";
mockIPC((command, payload) => {
  if (command !== "ipc_request") {
    if (command === "save_artifact" || command === "open_artifact") calls.push(`${command} ${(payload as { id: string }).id}`);
    return command === "save_artifact" ? true : null;
  }
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "resolveFileLink":
      calls.push(`${req.method} ${req.link}`);
      return { method: req.method, exists: false, stored: req.link.endsWith("notes.md") ? { taskId: "w1", artifact: NOTES } : null };
    case "readArtifact":
      calls.push(`${req.method} ${req.id}`);
      return { method: req.method, text: { text: NOTES_TEXT, offset: 0, totalBytes: NOTES_TEXT.length, binary: false } };
    case "getConversation":
      // `drive=loadfail`: the daemon can't give the view until the driver lets it.
      if (failLoads) throw new Error("The daemon did not answer.");
      return { method: req.method, view: view() };
    case "updateSettings":
      settings = req.settings;
      return { method: req.method, settings };
    case "listFiles":
      // `drive=filelink`: the checkout has the file its answer names, read back after a moment.
      if (drive === "filelink") return new Promise((resolve) => setTimeout(() => resolve({ method: req.method, files: ["CHANGELOG.md"], truncated: false }), 80));
      return { method: req.method, files: [], truncated: false };
    case "openSideChat":
      calls.push(`${req.method} ${req.conversationId}`);
      return { method: req.method, conversation: { ...conversation(SIDE, "Side chat", "chat"), sideOf: ID } };
    case "stopBackgroundRun":
      calls.push(`${req.method} ${req.runId}`);
      return { method: req.method };
    default:
      return { method: req.method };
  }
});

useApp.setState((state) => ({
  conversations,
  connection: { status: "connected", daemon: null, reason: null },
  // The background drive reads the context card beside the thread.
  summaryShown: { [ID]: drive === "background" },
  settings: { ...state.settings, followUp: "queue" },
}));
await openConversation(ID);

function Page() {
  const selection = useApp((s) => s.selection);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}
const app = createRoot(document.getElementById("root")!);
app.render(<Page />);

/* ----- driving and measuring ------------------------------------------------------------ */

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const one = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => root.querySelector<T>(selector);
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const turn = () => all("[data-role=assistant]").at(-1) ?? null;
const animating = () =>
  document.getAnimations().some((animation) => animation.playState === "running" && animation.effect?.getTiming().iterations !== Infinity);

/**
 * Waits `ms`, then until nothing animates (a shimmer's endless sweep aside), no reply waits for its
 * renderer's code (its placeholder pulses on and says nothing) and the turn's text holds for 100ms of frames.
 */
async function settle(ms = 50): Promise<void> {
  await wait(ms);
  const until = performance.now() + 8000;
  let last = text(turn());
  let stillSince = performance.now();
  while (performance.now() < until) {
    await frame();
    const now = text(turn());
    // A live label waiting its turn (at most a second), or a step's name lingering, still moves.
    const moving = animating() || one("[data-slot=reply-loading]") || one("[data-pending],[data-lingers]");
    if (moving || now !== last) stillSince = performance.now();
    else if (performance.now() - stillSince >= 100) return;
    last = now;
  }
}

/** Waits for what `find` finds, up to 8s, then goes on either way: the measures say what was missing. */
async function appear(find: () => Element | null): Promise<void> {
  const until = performance.now() + 8000;
  while (!find() && performance.now() < until) await frame();
}

/** Holds the page at the moment `stop` names, for a screenshot. */
async function hold(name: string): Promise<void> {
  if (query.get("stop") === name) await new Promise(() => {});
}

/** The board as the daemon's events would leave it. */
function change(edit: (board: Board) => Partial<Board>): void {
  useBoard.setState(({ board }) => (board ? { board: { ...board, ...edit(board) } } : {}));
}
function request(id: string, edit: Partial<UserRequest>): void {
  daemon.requests[id] = { ...daemon.requests[id]!, ...edit };
  change((board) => ({ requests: { ...board.requests, [id]: daemon.requests[id]! } }));
}
function steps(next: OrchestratorStep[]): void {
  daemon.steps = next;
  change(() => ({ orchestratorSteps: next }));
}

/** What the newest turn shows now. */
function seen() {
  const block = turn();
  const group = all("[data-slot=work-group]", block ?? document).at(-1);
  const work = all("[data-slot=request-work]", block ?? document);
  const answer = all("[data-slot=aui_assistant-message-content]", block ?? document).at(-1) ?? null;
  const status = one("[data-slot=thread-status]", block ?? document);
  // The newest thing in the live work: a group's row or a step's own.
  const newest = work.at(-1)?.lastElementChild ?? null;
  return {
    header: text(one("[data-slot=request-work-label]", block ?? document)),
    newest: text(newest),
    newestShimmers: !!newest && (newest.matches(".shimmer") || !!newest.querySelector(".shimmer")),
    fold: one("[data-slot=request-fold]", block ?? document)?.getAttribute("data-fold") ?? "gone",
    groupRow: text(one("button", group ?? document.createElement("div"))),
    // Its row's icons: a step's icon and the chevron, or the chevron alone.
    groupIcons: all("svg", one("button", group ?? document.createElement("div")) ?? document.createElement("div")).length,
    // What its words animate: the shimmer's sweep, never a fade that starts them invisible.
    groupAnimations: (one("button > span:not([data-icon])", group ?? document.createElement("div"))?.getAnimations() ?? []).map((animation) => (animation as CSSAnimation).animationName),
    groupLine: text(one("[data-slot=group-live-line]", block ?? document)),
    groupTone: one("[data-slot=group-live-line]", block ?? document)?.getAttribute("data-tone") ?? null,
    // The glyph before its words, by name ("tasks" for a question waiting for its answer).
    groupGlyph: one("button [data-icon]", group ?? document.createElement("div"))?.getAttribute("data-icon") ?? null,
    // Its live words' sweep runs over the fainter base of an approval's wait.
    groupQuiet: !!one("[data-slot=group-live-line].shimmer-quiet", block ?? document),
    shimmer: !!group?.querySelector(".shimmer"),
    status: text(status),
    thinkingLive: all("[data-slot=thinking-live]", block ?? document).length,
    commentary: text(one("[data-commentary]", block ?? document)),
    commentaryShimmers: !!one("[data-commentary] .shimmer, [data-commentary].shimmer", block ?? document),
    answer: text(answer),
    answerInWork: !!answer && work.some((part) => part.contains(answer)),
    statusAfterAnswer: !!answer && !!status && !!(answer.compareDocumentPosition(status) & Node.DOCUMENT_POSITION_FOLLOWING),
    statusInWork: !!status && work.some((part) => part.contains(status)),
  };
}

const thought = (complete: boolean): ThinkingSegment => ({
  itemId: "thought", position: 1.3, throughPosition: 1.3, requestId: "r1",
  text: "**Checking the failing test**\n\nThe rounding test expects half-up.", startedAtMs: Date.now() - 2000, updatedAtMs: Date.now(), complete,
});
const question: Question = {
  id: "ask", conversationId: ID, taskId: null, requestId: "r1", position: 1.05, kind: { type: "orchestrator" },
  text: "Round half-up or to even?", options: ["Half-up", "To even"], recommended: 0, items: [], answer: null, answers: [],
  createdAtMs: begun, answeredAtMs: null,
} as unknown as Question;

const approval = {
  id: "allow", conversationId: ID, taskId: null, requestId: "r1", position: 1.06, state: { type: "pending" }, createdAtMs: begun, resolvedAtMs: null,
  subject: { type: "cli", request: { id: "allow", kind: "command", tool: "Bash", command: "git push", cwd: null, paths: [], reason: null, escalation: false, input: null, grant: null } },
} as unknown as ShownApproval;

async function driveTurn(): Promise<object> {
  const out: Record<string, unknown> = {};
  // The header shows from the turn's first frame.
  for (const end = performance.now() + 8000; performance.now() < end && !turn(); ) await frame();
  out.firstHeader = text(one("[data-slot=request-work-label]", turn() ?? document));
  await settle(600);
  // Its newest group's step is done and nothing runs: the group's row says "Thinking".
  out.thinking = seen();
  change(() => ({ thinking: [thought(false)] }));
  await settle();
  out.thought = seen();
  // A question waits on the user: that wins over the thought's heading.
  change(() => ({ questions: { ask: question } }));
  await settle();
  out.question = seen();
  // A call waiting for the user's OK: the same row says so, fainter, with no glyph.
  change(() => ({ questions: {}, approvals: { allow: approval } }));
  await settle();
  out.approval = seen();
  change(() => ({ approvals: {}, thinking: [thought(true)] }));
  steps([...daemon.steps, tool("sleep", 1.4, "shell", "sleep 4", "inProgress")]);
  await settle();
  out.running = seen();
  await hold("running");
  // The command ends: its group still names it for a second, then says "Thinking".
  const ended = tool("sleep", 1.4, "shell", "sleep 4", "completed");
  if (ended.kind.type === "tool") ended.kind.endedAtMs = Date.now();
  steps(daemon.steps.map((step) => (step.kind.type === "tool" && step.kind.itemId === "sleep" ? ended : step)));
  await frame();
  out.justEnded = seen();
  await settle();
  out.ended = seen();
  // A line between steps streams as prose, each piece shown as it arrives: never paced.
  const line = "I’ll fix the rounding next.";
  change(() => ({ streaming: { messageId: "line", text: line, requestId: "r1", phase: "commentary", startedAtMs: Date.now() } }));
  await settle();
  const more = `${line}\n\nThen run the tests again.`;
  change(() => ({ streaming: { messageId: "line", text: more, requestId: "r1", phase: "commentary", startedAtMs: Date.now() } }));
  await frame();
  await frame();
  out.commentaryAtOnce = text(one("[data-commentary]", turn() ?? document));
  await settle();
  out.commentary = seen();
  // The final answer starts: the work folds at once, the time stops there. The fold is timed from
  // the first frame that shows the answer starting, by the frames' own clock: a slow machine
  // takes its time drawing that frame, which is not the fold's. What it took past its 150ms close
  // is counted in frames, which a loaded machine draws farther apart (#20).
  const answerAtMs = Date.now() - 1000;
  const answerText = "Fixed the rounding: `round` now rounds half-up, and the tests pass.";
  change(() => ({ streaming: { messageId: "a1", text: answerText.slice(0, 20), requestId: "r1", phase: "answer", startedAtMs: answerAtMs } }));
  await frame();
  const folding = Number(document.timeline.currentTime);
  out.foldFirst = one("[data-slot=request-fold]", turn() ?? document)?.getAttribute("data-fold") ?? "gone";
  let foldMs = -1;
  let foldLate = 0;
  for (const end = performance.now() + 2000; performance.now() < end; ) {
    const at = Number(document.timeline.currentTime) - folding;
    if (!one("[data-slot=request-fold]", turn() ?? document)) {
      foldMs = Math.round(at);
      break;
    }
    if (at >= 150) foldLate += 1;
    await frame();
  }
  out.foldMs = foldMs;
  out.foldLate = foldLate;
  await settle();
  out.answering = seen();
  out.announcedAnswering = text(one("[data-slot=answer-announcer]", turn() ?? document));
  // Ticks later it still says the same time.
  await wait(2200);
  change(() => ({ streaming: { messageId: "a1", text: answerText, requestId: "r1", phase: "answer", startedAtMs: answerAtMs } }));
  await settle();
  out.answeringLater = seen();
  // A question for the user while it streams: its line shows under the answer.
  change(() => ({ questions: { ask: question } }));
  await settle();
  out.answeringAsked = seen();
  change(() => ({ questions: {} }));
  await settle();
  await hold("answering");
  // Stored, the request still running: folded, the same time.
  const stored: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", text: answerText, blob: null, createdAtMs: Date.now(), attachments: [],
    mentions: [], model: null, requestId: "r1", parentId: "r1", answerAtMs,
  };
  daemon.messages = [...daemon.messages, stored];
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ streaming: null, head: "a1" }));
  await settle();
  out.stored = seen();
  // The turn ends: the request is done.
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null }));
  request("r1", { state: { type: "done" }, endedAtMs: Date.now() });
  await settle();
  out.done = seen();
  await hold("done");
  out.actions = all("[data-slot=answer-actions] button", turn() ?? document).map((button) => button.getAttribute("aria-label") ?? text(one(".sr-only", button)));
  // The latest answer says when it came without a hover; a screen reader heard it start and end.
  const time = one("[data-slot=answer-time]", turn() ?? document);
  out.time = { text: text(time), opacity: time ? getComputedStyle(time).opacity : null };
  out.announcedDone = text(one("[data-slot=answer-announcer]", turn() ?? document));
  // Copy's rich text: the answer as shown, with no classes or controls.
  // A reply of the work, as an opened fold shows it, stays out of it.
  const work = document.createElement("div");
  work.setAttribute("data-slot", "aui_assistant-message-content");
  work.setAttribute("data-commentary", "true");
  work.textContent = "I’ll fix the rounding next.";
  one("[data-slot=request-body]", turn() ?? document)?.prepend(work);
  out.html = turn() ? answerHtml(turn()!) : null;
  work.remove();
  // Read again from the daemon, as after a restart.
  await loadConversation(ID);
  await settle(300);
  out.reloaded = seen();
  return out;
}

/** A rail narrow enough that a queued row's message and its held note can't both show whole. */
const NARROW_RAIL_PX = 560;

async function driveSteer(): Promise<object> {
  await settle(600);
  steps([tool("sleep", 1.1, "shell", "sleep 30", "inProgress")]);
  await settle();
  const before = seen();
  // A follow-up sent meanwhile waits behind the command rather than deciding on and on.
  const held = { command: "sleep 30", sinceMs: Date.now() - 2 * 60_000 };
  change(() => ({ queue: { items: [{ ...queued, deciding: true, heldBy: held }], paused: false } }));
  await settle();
  const deciding = { note: text(one("[data-slot=message-queue-item] [data-slot=held-note]")), shimmer: text(one("[data-slot=message-queue-item] .shimmer")) };
  // #447: on a narrow rail the message stays readable; the note gives way first.
  const list = one("[data-slot=message-queue]");
  if (list) list.style.width = `${NARROW_RAIL_PX}px`;
  await settle();
  const message = one("[data-slot=message-queue-item] [data-slot=queue-item-text]");
  const note = one("[data-slot=message-queue-item] [data-slot=held-note]");
  const narrow = {
    messageShown: message ? message.clientWidth / message.scrollWidth : null,
    noteWidth: note?.clientWidth ?? null,
  };
  if (list) list.style.width = "";
  change(() => ({ queue: { items: [], paused: false } }));
  // The user steers a message in while the command runs.
  const steered = user("r2", 2, "Use banker's rounding instead", Date.now());
  daemon.messages = [...daemon.messages, steered];
  daemon.requests.r2 = asked("r2", Date.now(), { steeredInto: "r1" });
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change((board) => ({ requests: { ...board.requests, r2: daemon.requests.r2! }, head: "r2" }));
  await settle();
  const after = seen();
  // #445: the command above the bubble still runs, and its row says so.
  const commandRow = () => text(all("[data-slot=request-work]", turn() ?? document)[0]?.lastElementChild ?? null);
  const stillRuns = commandRow();
  const bubble = text(one("[data-slot=request-steer]"));
  // Not read yet: the thread is inside the command.
  request("r2", { heldBy: held });
  await settle();
  const steerHeld = text(one("[data-slot=request-steer] [data-slot=held-note]"));
  await hold("steer");
  // The command ended: the thread reads it, the note goes.
  const { heldBy: _read, ...read } = daemon.requests.r2!;
  daemon.requests.r2 = read;
  change((board) => ({ requests: { ...board.requests, r2: read } }));
  await settle();
  const steerRead = text(one("[data-slot=request-steer] [data-slot=held-note]"));
  // Once it ends, the work above the bubble sums up.
  steps([tool("sleep", 1.1, "shell", "sleep 30", "completed")]);
  await settle();
  return { before, after, stillRuns, ended: commandRow(), bubble, deciding, narrow, steerHeld, steerRead };
}

/** A message steered in before the turn did anything: its bubble sits under the first, above the header. */
async function driveEarly(): Promise<object> {
  await settle(600);
  const steered = user("r2", 2, "Use banker's rounding instead", Date.now());
  daemon.messages = [...daemon.messages, steered];
  daemon.requests.r2 = asked("r2", Date.now(), { steeredInto: "r1" });
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change((board) => ({ requests: { ...board.requests, r2: daemon.requests.r2! }, head: "r2" }));
  await settle();
  const block = turn() ?? document.body;
  const bubble = one("[data-slot=request-steer]", block);
  const header = one("[data-slot=request-work-header]", block);
  const above = !!bubble && !!header && !!(bubble.compareDocumentPosition(header) & Node.DOCUMENT_POSITION_FOLLOWING);
  await hold("early");
  // Work comes after it: the bubble stays where it was, the rows go under the header.
  steps([tool("read", 2.1, "Read", "/repo/src/math.js", "completed")]);
  await settle();
  const header2 = one("[data-slot=request-work-header]", turn() ?? document);
  const bubble2 = one("[data-slot=request-steer]", turn() ?? document);
  return {
    bubble: text(bubble), above, header: text(header), status: text(one("[data-slot=group-live-line], [data-slot=thread-status]", block)),
    stillAbove: !!bubble2 && !!header2 && !!(bubble2.compareDocumentPosition(header2) & Node.DOCUMENT_POSITION_FOLLOWING),
  };
}

/** A turn whose answer starts while the window is hidden folds its work at once: WebKit holds animations still there. */
async function driveHidden(): Promise<object> {
  await settle(600);
  const shown = !!one("[data-slot=request-fold]", turn() ?? document);
  Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
  change(() => ({ streaming: { messageId: "a1", text: "Fixed the rounding.", requestId: "r1", phase: "answer", startedAtMs: Date.now() } }));
  await frame();
  return { shown, gone: !one("[data-slot=request-fold]", turn() ?? document) };
}

/** A turn that ends in a plan keeps its work open; the user's choice outlives the turn's remount. */
async function drivePlan(): Promise<object> {
  await settle(600);
  const plan = {
    id: "plan", conversationId: ID, requestId: "r1", position: 1.5, title: "Round half-up",
    body: "# Round half-up\n\nUse half-up rounding in `round`.\n\n## Steps\n\n1. Change `round`.\n2. Add a test.", steps: [],
    state: { type: "proposed" }, createdAtMs: begun + 10_000, decidedAtMs: null,
  } as unknown as Plan;
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null, plans: { plan } }));
  request("r1", { state: { type: "done" }, endedAtMs: begun + 10_000, worked: [{ fromMs: begun, toMs: begun + 10_000 }] });
  await settle();
  const ended = seen();
  one<HTMLButtonElement>("button[data-slot=request-work-header]", turn() ?? document)?.click();
  await settle();
  const closed = seen();
  // Gone and back (a thread switch): the turn stays as the user left it.
  app.render(<></>);
  await settle();
  app.render(<Page />);
  await settle(600);
  const remounted = seen();
  // Full screen, the plan is its own tab in the tab bar, never the right panel.
  const full = [...document.querySelectorAll<HTMLButtonElement>("[data-slot=plan-card] button")].find((button) => text(button) === "Open full screen");
  full?.click();
  await settle();
  const planTab = () => one('[role=tabpanel][aria-label="Round half-up"] [data-slot=plan-tab]');
  // The tab's view loads its code the first time one opens: the tab shows nothing until then.
  if (full) await appear(planTab);
  const tabs = sessionTabs(ID);
  const panel = planTab();
  const opened = {
    canOpen: one("[data-slot=plan-card] [role=button]") !== null,
    active: tabs.active, kinds: tabs.tabs.map((tab) => tab.kind), shown: text(panel)?.includes("Add a test.") ?? false,
    // The conversation's own tabs: the Workers panel's tab is not one.
    tab: text(one('[role=tab][aria-selected=true]:not([id^="right-sidebar-"])')), rightPanel: one('[data-slot="right-sidebar"] [data-pane="plan"]') !== null,
  };
  if (tabs.active !== "chat") closeTab(ID, tabs.active);
  return { ended, closed, remounted, opened };
}

/** A finished turn with a call the user denied: "1 denied action" in its header shows it. */
async function driveDenied(): Promise<object> {
  await settle(600);
  const push = { ...tool("push", 1.3, "Bash", "git push origin main", "completed"), kind: { type: "tool", itemId: "push", name: "Bash", status: "failed", detail: "git push origin main", throughPosition: 1.3 } } as OrchestratorStep;
  steps([...daemon.steps, push]);
  const deny = { ...approval, id: "deny", state: { type: "denied" }, resolvedAtMs: begun + 1500 } as unknown as ShownApproval;
  const answer: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", blob: null, createdAtMs: begun + 5000, attachments: [], mentions: [],
    text: "I left the push to you.", model: null, requestId: "r1", parentId: "r1", answerAtMs: begun + 5000,
  };
  daemon.messages = [...daemon.messages, answer];
  daemon.run = "idle";
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ run: "idle", runRequest: null, approvals: { deny } }));
  request("r1", { state: { type: "done" }, endedAtMs: begun + 5000, worked: [{ fromMs: begun, toMs: begun + 5000 }] });
  await settle();
  const before = { denied: text(one("[data-slot=request-denied]", turn() ?? document)), fold: seen().fold };
  one<HTMLButtonElement>("[data-slot=request-denied]", turn() ?? document)?.click();
  await settle(600);
  const row = one("[data-step-state=declined]", turn() ?? document);
  return { before, fold: seen().fold, row: text(row) };
}

/** A long message: clipped at the end of the code block crossing its 20-line cut, "Show more" with a chevron. */
async function driveClip(): Promise<object> {
  await settle(600);
  for (const end = performance.now() + 8000; performance.now() < end && !one(".aui-user-message-content pre"); ) await frame();
  await settle();
  const bubble = one(".aui-user-message-content");
  const clip = bubble?.firstElementChild as HTMLElement | null;
  const block = one(".aui-user-message-content pre");
  // A font come late lays the block out again; the cut follows on the next frame: read once settled.
  await document.fonts.ready;
  const apart = () => Math.abs((clip?.getBoundingClientRect().bottom ?? 0) - (block?.getBoundingClientRect().bottom ?? 0));
  for (const end = performance.now() + 2000; performance.now() < end && apart() > 1; ) await frame();
  const more = all("button", bubble ?? document).find((button) => text(button) === "Show more");
  const lineHeight = clip ? Number.parseFloat(getComputedStyle(clip).lineHeight) : 0;
  return {
    clipEnd: clip ? Math.round(clip.getBoundingClientRect().bottom) : null,
    blockEnd: block ? Math.round(block.getBoundingClientRect().bottom) : null,
    clipLines: clip && lineHeight ? clip.getBoundingClientRect().height / lineHeight : null,
    more: !!more,
    chevron: !!more?.querySelector("svg"),
    sides: clip ? [getComputedStyle(clip).overflowX, getComputedStyle(clip).overflowY] : null,
  };
}

/** The user's message edited in place: a composer the column's width, Cancel and Send as pills. */
async function driveEdit(): Promise<object> {
  await settle(600);
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null }));
  request("r1", { state: { type: "done" }, endedAtMs: begun + 5000, worked: [{ fromMs: begun, toMs: begun + 5000 }] });
  await settle();
  // A screen reader's "Latest response" moves the focus to the newest answer.
  one<HTMLButtonElement>("[data-slot=latest-response]")?.click();
  const latest = document.activeElement?.getAttribute("data-role") ?? null;
  const edit = one<HTMLButtonElement>(".aui-user-action-edit");
  edit?.click();
  await settle();
  const box = one("[data-slot=edit-composer]");
  const buttons = all<HTMLButtonElement>("button", box ?? document.createElement("div"));
  const pill = (name: string) => {
    const button = buttons.find((each) => text(each) === name);
    if (!button) return null;
    const style = getComputedStyle(button);
    return { round: Number.parseFloat(style.borderTopLeftRadius) >= button.getBoundingClientRect().height / 2, border: Number.parseFloat(style.borderTopWidth) };
  };
  const column = one("[data-slot=aui_thread-column]");
  return {
    latest,
    edit: !!edit,
    // The same fill as the composer below the thread.
    fill: box ? getComputedStyle(box).backgroundColor === getComputedStyle(all(".bg-composer").find((each) => each !== box) ?? document.body).backgroundColor : null,
    radius: box ? Number.parseFloat(getComputedStyle(box).borderTopLeftRadius) : null,
    wide: box && column ? box.getBoundingClientRect().width / column.getBoundingClientRect().width : null,
    cancel: pill("Cancel"),
    send: pill("Send"),
  };
}

/** The thread's messages fail to load: "Couldn't load messages" with Retry, which loads them. */
async function driveLoadFail(): Promise<object> {
  for (const end = performance.now() + 8000; performance.now() < end && !one("[data-slot=load-error]"); ) await frame();
  const failed = { line: text(one("[data-slot=load-error]")), role: one("[data-slot=load-error]")?.getAttribute("role") ?? null, rows: all("[data-message-id]").length };
  failLoads = false;
  all<HTMLButtonElement>("[data-slot=load-error] button").find((button) => text(button) === "Retry")?.click();
  for (const end = performance.now() + 8000; performance.now() < end && one("[data-slot=load-error]"); ) await frame();
  await settle();
  return { failed, after: { line: text(one("[data-slot=load-error]")), rows: all("[data-message-id]").length } };
}

/** Text selected in an answer: the pill over it adds it to the chat or asks about it in a side chat. */
async function driveSelect(): Promise<object> {
  await settle(600);
  const answer: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", blob: null, createdAtMs: begun + 5000, attachments: [], mentions: [],
    text: "Rounding now uses half-up everywhere.\n\nThe tests cover negative values too.", model: null, requestId: "r1", parentId: "r1", answerAtMs: begun + 5000,
  };
  daemon.messages = [...daemon.messages, answer];
  daemon.run = "idle";
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ run: "idle", runRequest: null, head: "a1" }));
  request("r1", { state: { type: "done" }, endedAtMs: begun + 5000, worked: [{ fromMs: begun, toMs: begun + 5000 }] });
  for (const end = performance.now() + 8000; performance.now() < end && !all("[data-role=assistant] p").some((p) => text(p)?.startsWith("Rounding")); ) await frame();
  await settle();
  const select = () => {
    const paragraph = all("[data-role=assistant] p").find((p) => text(p)?.startsWith("Rounding"));
    if (!paragraph) throw new Error("the answer's paragraph never showed");
    const range = document.createRange();
    range.selectNodeContents(paragraph);
    const selection = document.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    return range.getBoundingClientRect();
  };
  const pill = () => one("[data-slot=selection-pill]");
  const box = select();
  for (const end = performance.now() + 3000; performance.now() < end && !pill(); ) await frame();
  const shown = pill();
  const placed = shown ? { bottomGap: Math.round(box.top - shown.getBoundingClientRect().bottom), centre: Math.round(shown.getBoundingClientRect().left + shown.getBoundingClientRect().width / 2 - (box.left + box.width / 2)) } : null;
  const buttons = all("button", shown ?? document.createElement("div")).map((button) => text(button));
  const pills = all("[data-slot=selection-pill]").length;
  // Another chat's composer earlier in the page (a side-by-side one): the pill's own takes the focus.
  const decoy = document.createElement("div");
  decoy.setAttribute("data-slot", "composer-input");
  decoy.innerHTML = '<div class="aui-lexical-input" tabindex="-1"></div>';
  document.body.prepend(decoy);
  all<HTMLButtonElement>("button", shown ?? document.createElement("div")).find((button) => text(button) === "Add to chat")?.click();
  await settle();
  const focusedOwn = document.activeElement?.matches("[data-slot=composer-input] .aui-lexical-input") === true && !decoy.contains(document.activeElement);
  decoy.remove();
  // The rich field shows the quote as a quote block.
  const composer = text(one("[contenteditable=true][aria-label='Message input'] blockquote"));
  const marked = CSS.highlights.get("thread-annotation")?.size ?? 0;
  const afterAdd = { pill: !!pill(), collapsed: document.getSelection()?.isCollapsed ?? null };
  // Selected again: Ask in side chat.
  select();
  for (const end = performance.now() + 3000; performance.now() < end && !pill(); ) await frame();
  all<HTMLButtonElement>("button", pill() ?? document.createElement("div")).find((button) => text(button) === "Ask in side chat")?.click();
  for (const end = performance.now() + 3000; performance.now() < end && !calls.some((call) => call.startsWith("openSideChat")); ) await frame();
  await settle();
  const draft = JSON.parse(localStorage.getItem("brigadier.drafts") ?? "{}")[SIDE]?.text ?? null;
  return { pills, placed, buttons, composer, focusedOwn, marked, afterAdd, sideChat: calls.filter((call) => call.startsWith("openSideChat")), draft };
}

/**
 * A finished answer with a file link, then a later turn's live updates (steps, a thought, its
 * streaming line): the answer is never drawn again, so its chip stays the same node, styled.
 */
async function driveFileLink(): Promise<object> {
  await settle(600);
  const answer: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", blob: null, createdAtMs: begun + 5000, attachments: [], mentions: [],
    text: "I added `CHANGELOG.md` with an entry.\n\n- one\n- two", model: null, requestId: "r1", parentId: "r1", answerAtMs: begun + 5000,
  };
  daemon.messages = [...daemon.messages, answer];
  daemon.run = "idle";
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ run: "idle", runRequest: null, head: "a1" }));
  request("r1", { state: { type: "done" }, endedAtMs: begun + 5000, worked: [{ fromMs: begun, toMs: begun + 5000 }] });
  for (const end = performance.now() + 8000; performance.now() < end && !one("[data-slot=file-chip]"); ) await frame();
  await settle(300);
  const chip = one("[data-slot=file-chip]");
  const events: string[] = [];
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.removedNodes) {
        if (node instanceof Element && (node.matches("[data-slot=file-chip]") || node.querySelector("[data-slot=file-chip]"))) events.push("chip removed");
      }
      for (const node of record.addedNodes) {
        if (!(node instanceof Element)) continue;
        if (node.matches("[data-slot=reply-loading], p.whitespace-pre-wrap") || node.querySelector("[data-slot=reply-loading], p.whitespace-pre-wrap")) events.push(`fallback ${node.className}`);
      }
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
  // A follow-up turn runs: its steps come one by one, then a line between them streams.
  const second = user("r2", 3, "Also add a test", Date.now());
  daemon.messages = [...daemon.messages, second];
  daemon.requests.r2 = asked("r2", Date.now());
  daemon.run = "running";
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change((board) => ({ run: "running", runRequest: "r2", head: "r2", requests: { ...board.requests, r2: daemon.requests.r2! } }));
  await settle(200);
  for (let index = 0; index < 4; index += 1) {
    const step = { ...tool(`s${index}`, 3 + index / 10, "Read", `/repo/src/file${index}.js`, "inProgress"), requestId: "r2" } as OrchestratorStep;
    // Each event moves the conversation's time, as the daemon's do.
    const touch = () => useApp.setState((state) => ({ conversations: { ...state.conversations, [ID]: { ...state.conversations[ID]!, updatedAtMs: Date.now() } } }));
    steps([...daemon.steps, step]);
    touch();
    // Another event before the checkout's files are read back.
    await frame();
    touch();
    await wait(120);
    steps(daemon.steps.map((s) => (s === step ? ({ ...step, kind: { ...step.kind, status: "completed" } } as OrchestratorStep) : s)));
    touch();
    await wait(120);
  }
  change(() => ({ streaming: { messageId: "line2", text: "Now the test.", requestId: "r2", phase: "commentary", startedAtMs: Date.now() } }));
  await settle(300);
  observer.disconnect();
  const after = one("[data-slot=file-chip]");
  return { same: !!chip && chip === after, connected: chip?.isConnected ?? false, events, color: after ? getComputedStyle(after).color : null };
}

/**
 * A finished answer names its worker's file once, inline (Q84): no files under the answer and no
 * Save by the link; the link opens the file in the reader as the answer shows Markdown, and Save is there.
 */
async function driveAnswerFile(): Promise<object> {
  await settle(600);
  // The worker that wrote the file, of this request: it once put the file under the answer too.
  const worker = {
    ...(Object.values(night.tasks)[0] as unknown as Task), id: "w1", number: 1, title: "Rounding cases", conversationId: ID, state: "done",
    blockedReason: null, role: null, phase: null, run: null, requestId: "r1", gateLink: null, candidate: null, report: null, quotaWait: null,
    position: 1.3, createdAtMs: begun + 2000, updatedAtMs: begun + 4000, outputs: [NOTES],
  } as unknown as Task;
  const answer: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", blob: null, createdAtMs: begun + 5000, attachments: [], mentions: [],
    text: "Totals now round half-even, once, after tax. The cases I checked are in [notes.md](outputs/task-1/notes.md); the invoice test covers the two that failed.",
    model: null, requestId: "r1", parentId: "r1", answerAtMs: begun + 5000,
  };
  daemon.messages = [...daemon.messages, answer];
  daemon.run = "idle";
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ run: "idle", runRequest: null, head: "a1", tasks: { w1: worker } }));
  request("r1", { state: { type: "done" }, endedAtMs: begun + 5000, worked: [{ fromMs: begun, toMs: begun + 5000 }] });
  await appear(() => one("[data-slot=file-chip]", turn() ?? document.body));
  await settle(300);
  const chips = all("[data-slot=file-chip]", turn() ?? document.body);
  const link = chips[0]?.closest("p") ?? null;
  const shown = { underAnswer: !!one("[data-slot=answer-files], [data-slot=answer-file]"), chips: chips.map(text), linkLine: text(link), saveByLink: /Save/.test(text(link) ?? "") };
  await hold("answer");
  await fixtureClick(chips[0]!);
  await appear(() => one("[data-slot=file-reader] [data-slot=file-reader-body] h1"));
  await settle(200);
  const reader = one("[data-slot=file-reader]");
  const opened = {
    title: text(one("h2", reader ?? document.body)),
    heading: text(one("[data-slot=file-reader-body] h1", reader ?? document.body)),
    items: all("[data-slot=file-reader-body] li", reader ?? document.body).map(text),
    save: text(one("[data-slot=file-reader-save]", reader ?? document.body)),
  };
  await hold("reader");
  await fixtureClick(one("[data-slot=file-reader-save]")!);
  await appear(() => (calls.some((call) => call.startsWith("save_artifact")) ? document.body : null));
  return { ...shown, opened, calls: [...calls] };
}

/**
 * A follow-up that joined a turn as it ended (queued, it ran in the same turn): once all is done,
 * the first request's answer stays in view under its fold, above the follow-up's bubble.
 */
async function driveJoined(): Promise<object> {
  await settle(600);
  const first: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", blob: null, createdAtMs: begun + 5000, attachments: [], mentions: [],
    text: "Each line's tax is now rounded to cents.", model: null, requestId: "r1", parentId: "r1",
  };
  const followUp = user("r2", 3, "Also add a CHANGELOG entry", begun + 6000);
  daemon.messages = [...daemon.messages, first, followUp];
  daemon.requests.r2 = asked("r2", begun + 6000, { steeredInto: "r1" });
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change((board) => ({ requests: { ...board.requests, r2: daemon.requests.r2! }, head: "r2" }));
  await settle();
  steps([...daemon.steps, { ...tool("edit", 3.1, "Edit", "/repo/CHANGELOG.md", "completed"), requestId: "r2" } as OrchestratorStep]);
  // The follow-up starts a worker: a team sentence among its work, as any worker row.
  const worker = (state: Task["state"]): Task => ({
    ...(Object.values(night.tasks)[0] as unknown as Task), id: "w1", number: 1, title: "Changelog check", conversationId: ID, state,
    blockedReason: null, role: null, phase: null, run: null, attempts: [{ startedAtMs: Date.now() - 2000, endedAtMs: null } as unknown as Task["attempts"][number]],
    requestId: "r2", gateLink: null, candidate: null, report: null, quotaWait: null, position: 3.2, createdAtMs: Date.now() - 2000, updatedAtMs: Date.now(),
  } as Task);
  change(() => ({ tasks: { w1: worker("running") }, workerSteps: [{ taskId: "w1", requestId: "r2", kind: "started", atMs: Date.now(), position: 3.2 }] }));
  await settle(300);
  const liveRow = one("[data-slot=task-row][data-kind=started]", turn() ?? document.body);
  const workerLive = { shown: !!liveRow && visible(liveRow), words: text(liveRow), card: !!one("[data-slot=worker-card]") };
  change((board) => ({
    tasks: { w1: worker("done") },
    workerSteps: [...board.workerSteps, { taskId: "w1", requestId: "r2", kind: "finished", atMs: Date.now(), position: 3.3 }],
  }));
  const second: Message = {
    id: "a2", conversationId: ID, seq: 4, role: "assistant", blob: null, createdAtMs: begun + 9000, attachments: [], mentions: [],
    text: "I added a CHANGELOG entry.", model: null, requestId: "r2", parentId: "r2",
  };
  daemon.messages = [...daemon.messages, second];
  daemon.run = "idle";
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ run: "idle", runRequest: null, head: "a2" }));
  request("r2", { state: { type: "done" }, endedAtMs: begun + 9000, worked: [{ fromMs: begun + 6000, toMs: begun + 9000 }] });
  request("r1", { state: { type: "done" }, endedAtMs: begun + 9000, worked: [{ fromMs: begun, toMs: begun + 9000 }] });
  await settle(300);
  const block = turn() ?? document.body;
  const shown = (words: string) =>
    all("[data-slot=aui_assistant-message-content]", block).some((element) => text(element)?.includes(words) && element.getClientRects().length > 0 && !element.closest("[data-slot=request-fold][data-fold=closed], [hidden]"));
  const firstAnswer = all("[data-slot=joined-answer]", block)[0] ?? null;
  const bubble = one("[data-slot=request-steer]", block);
  const rows = all("[data-slot=task-row]", block);
  const firstActions = firstAnswer ? one("[data-slot=answer-actions]", firstAnswer) : null;
  return {
    workerLive,
    // Answered, the worker's rows are work like the rest: under the closed fold, no card.
    workerDone: {
      rows: rows.map((row) => text(row)),
      shown: rows.some(visible),
      card: !!one("[data-slot=worker-card]"),
    },
    // The answer the follow-up joined has its own actions, shown without a hover.
    firstActions: all("button", firstActions ?? document.createElement("div")).map((button) => button.getAttribute("aria-label") ?? text(one(".sr-only", button))),
    firstActionsOpacity: firstActions ? getComputedStyle(firstActions).opacity : null,
    first: shown("rounded to cents"),
    second: shown("I added a CHANGELOG entry"),
    firstAboveBubble: !!firstAnswer && !!bubble && !!(firstAnswer.compareDocumentPosition(bubble) & Node.DOCUMENT_POSITION_FOLLOWING),
    header: text(one("[data-slot=request-work-label]", block)),
  };
}

/** Whether an element shows: drawn, and not inside a closed fold. */
const visible = (element: Element) => element.getClientRects().length > 0 && !element.closest("[data-slot=request-fold][data-fold=closed], [hidden]");

/**
 * An ending that opens with "Update:" (the thread reports again after a late result) and writes
 * its own `### Details`: the answer, never an update row under the opening line (#231, #198).
 */
async function driveUpdate(): Promise<object> {
  await settle(600);
  const reply = (id: string, seq: number, parentId: string, words: string): Message => ({
    id, conversationId: ID, seq, role: "assistant", blob: null, createdAtMs: begun + seq * 1000, attachments: [], mentions: [],
    text: words, model: null, requestId: "r1", parentId,
  });
  daemon.messages = [
    ...daemon.messages,
    reply("a1", 2, "r1", "Starting on the rounding fix."),
    reply("a2", 3, "a1", "Update: the session's full checks still don't pass, so it isn't merged.\n\nThe rounding fix itself is in.\n\n### Details\n- Unit tests pass.\n- The full checks fail in `billing`."),
  ];
  daemon.run = "idle";
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ run: "idle", runRequest: null, head: "a2" }));
  // The session's rounding fix changed code, so its Details stay; a Chat changes none, so it has none (Q79).
  const changes = { stat: { files: [{ path: "src/math.js", insertions: 2, deletions: 1, binary: false }], insertions: 2, deletions: 1 }, settled: true };
  request("r1", { state: { type: "done" }, endedAtMs: begun + 4000, worked: [{ fromMs: begun, toMs: begun + 4000 }], ...(session ? { changes } : {}) } as Partial<UserRequest>);
  await settle(300);
  const block = turn() ?? document.body;
  const details = all<HTMLDetailsElement>("[data-slot=report-details]", block);
  return {
    // What reads: a closed fold's words don't.
    answers: all("[data-answer]", block).filter(visible).map((element) => element.innerText.replace(/\s+/g, " ").trim()),
    updates: all("[data-slot=answer-update]", block).length,
    quotes: all("blockquote", block).filter(visible).length,
    details: details.map((element) => ({ summary: text(one("summary", element)), open: element.open })),
    opening: all("[data-slot=aui_assistant-message-content]", block).some((element) => visible(element) && !!text(element)?.startsWith("Starting on")),
  };
}

/** A plan approved: its request works again, and the new steps show under the plan card as they come. */
async function driveImplement(): Promise<object> {
  await settle(600);
  const plan = {
    id: "plan", conversationId: ID, requestId: "r1", position: 1.5, title: "Round half-up",
    body: "# Round half-up\n\nUse half-up rounding in `round`.", steps: [],
    state: { type: "proposed" }, createdAtMs: begun + 10_000, decidedAtMs: null,
  } as unknown as Plan;
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null, plans: { plan } }));
  request("r1", { state: { type: "done" }, endedAtMs: begun + 10_000, worked: [{ fromMs: begun, toMs: begun + 10_000 }] });
  await settle();
  const before = text(turn()) ?? "";
  // Approved: the same request works on.
  change(() => ({ run: "running", runRequest: "r1", plans: { plan: { ...plan, state: { type: "approved" } } as unknown as Plan } }));
  request("r1", { state: { type: "working" }, endedAtMs: null, worked: [{ fromMs: begun, toMs: begun + 10_000 }, { fromMs: Date.now(), toMs: null }] });
  // Its new steps come now, after the plan.
  const now = (step: OrchestratorStep): OrchestratorStep => ({ ...step, atMs: Date.now() });
  steps([...daemon.steps, now(tool("impl-read", 3.1, "Read", "/repo/src/cli.ts", "completed")), now(tool("impl-edit", 3.2, "apply_patch", "/repo/src/cli.ts", "inProgress"))]);
  await settle();
  const block = turn() ?? document.body;
  const card = one("[data-slot=plan-card], [data-card=plan], [data-slot=action-card]", block);
  const after = text(block) ?? "";
  const planAt = after.indexOf("Round half-up");
  return { before, after, card: !!card, under: planAt >= 0 && after.lastIndexOf("cli.ts") > planAt, new: !before.includes("cli.ts") };
}

async function driveQueue(): Promise<object> {
  await settle(600);
  // The composer's editor comes in its own chunk after a plain field and takes the focus a beat
  // after it shows, which closes a menu open by then: on a slow machine that comes after the turn
  // holds still, so the menu waits for the editor to hold the focus.
  const editing = () => one("[contenteditable=true][aria-label='Message input']:focus");
  for (const end = Date.now() + 8000; Date.now() < end && !editing(); ) await frame();
  const trigger = one<HTMLButtonElement>("[aria-label='Queued message actions']");
  trigger?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, pointerType: "mouse" }));
  await settle();
  const items = all("[role=menuitem]").map((item) => text(item));
  await hold("menu");
  all("[role=menuitem]").find((item) => text(item) === "Turn off queueing")?.click();
  for (const end = Date.now() + 3000; Date.now() < end && settings.followUp !== "steer"; ) await wait(25);
  return { items, followUp: useApp.getState().settings.followUp, saved: settings.followUp };
}

async function driveBackground(): Promise<object> {
  await settle(600);
  // The thread's command outlasted its call: it went to the background, and the thread said so.
  steps([...daemon.steps, tool("check", 1.3, "mcp__brigadier__run", "cargo test --workspace", "completed")]);
  const said: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", blob: null, createdAtMs: Date.now(), attachments: [], mentions: [],
    text: "The full Rust tests run in the background, about 15 minutes; I'll answer when they finish.", model: null,
    requestId: "r1", parentId: "r1", answerAtMs: Date.now(),
  };
  daemon.messages = [...daemon.messages, said];
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  const run: BackgroundRun = {
    id: "run-1", conversationId: ID, requestId: "r1", tool: "run", command: "cargo test --workspace",
    startedAtMs: Date.now() - 6 * 60_000 - 5_000, endedAtMs: null, state: { type: "running" },
  };
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null, head: "a1", backgroundRuns: { [run.id]: run } }));
  await settle(300);
  const card = () => one("[data-activity=run-1]");
  const running = {
    ...seen(),
    said: all("[data-slot=request-work]", turn() ?? document).some((part) => text(part)?.includes("about 15 minutes")),
    row: text(card()),
    elapsed: text(one("[data-activity=run-1] span.tabular-nums")),
  };
  await hold("background");
  // The row's Stop, in its time's place on hover.
  one<HTMLButtonElement>("[data-slot=summary-row-action] button", card()?.parentElement ?? document.createElement("div"))?.click();
  for (const end = Date.now() + 3000; Date.now() < end && calls.length === 0; ) await wait(25);
  // It stopped and the thread answered: the request is done, the run's row gone.
  const answer: Message = { ...said, id: "a2", seq: 3, text: "The full Rust tests pass: 812 tests, no failures.", createdAtMs: Date.now(), answerAtMs: Date.now() };
  daemon.messages = [...daemon.messages, answer];
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ head: "a2", backgroundRuns: { [run.id]: { ...run, endedAtMs: Date.now(), state: { type: "stopped" } } } }));
  request("r1", { state: { type: "done" }, endedAtMs: Date.now(), worked: [{ fromMs: begun, toMs: Date.now() }] });
  await settle(300);
  return { running, calls, done: { ...seen(), row: text(card()) } };
}

async function driveTimer(): Promise<object> {
  await settle(600);
  // 10 s of work, then its question card waits on the user.
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null, questions: { ask: question } }));
  request("r1", {
    state: { type: "waiting" }, endedAtMs: begun + 10_000,
    worked: [{ fromMs: begun, toMs: begun + 10_000 }], waited: [{ fromMs: begun + 10_000, toMs: null }],
  });
  await settle();
  const asking = seen();
  // Answered 15 s later; 5 s more of work and it is done.
  change(() => ({ questions: {} }));
  request("r1", {
    state: { type: "done" }, endedAtMs: begun + 30_000,
    worked: [{ fromMs: begun, toMs: begun + 10_000 }, { fromMs: begun + 25_000, toMs: begun + 30_000 }],
    waited: [{ fromMs: begun + 10_000, toMs: begun + 25_000 }],
  });
  await settle();
  const done = seen();
  // One that never waited on a card shows its work alone.
  request("r1", { waited: [] });
  await settle();
  return { asking, done, noWait: seen() };
}

void (drive === "implement" ? driveImplement() : drive === "joined" ? driveJoined() : drive === "update" ? driveUpdate() : drive === "filelink" ? driveFileLink() : drive === "answerfile" ? driveAnswerFile() : drive === "timer" ? driveTimer() : drive === "steer" ? driveSteer() : drive === "early" ? driveEarly() : drive === "hidden" ? driveHidden() : drive === "plan" ? drivePlan() : drive === "denied" ? driveDenied() : drive === "clip" ? driveClip() : drive === "edit" ? driveEdit() : drive === "loadfail" ? driveLoadFail() : drive === "select" ? driveSelect() : drive === "queue" ? driveQueue() : drive === "background" ? driveBackground() : driveTurn())
  .catch((error: unknown) => ({ error: String(error) }))
  .then((result) => {
    const out = document.createElement("pre");
    out.id = "work-blocks-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
