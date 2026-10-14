/**
 * A standalone Vite entry, excluded from the app entry and production build: one turn's work
 * block in the app's own conversation view, driven through the moments of a turn by changing its
 * board as the daemon's events would, each measured once the page holds still.
 *
 * - `drive=turn`: a Chat's turn reads a file and runs the tests; then it thinks, a question waits
 *   on the user, a command runs, a line between steps streams, the final answer streams, is
 *   stored, the request rests waiting for another session's reply, it is done, and the board is
 *   read again from the daemon.
 * - `drive=steer`: a message steered in while a command runs.
 * - `drive=queue` (`kind=session` for a session): a queued message's ⋯ menu, and turning
 *   queueing off from it.
 * - `drive=background` (with `kind=session`): the thread's check goes to the background and it
 *   says so; the request works on with a live line and a context-card row (Stop) until the
 *   result's answer.
 *
 * The result goes to `<pre id="work-blocks-result">`. `stop=running|answering|done|steer|menu` holds the
 * page at that moment instead, for a screenshot.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  BackgroundRun,
  Conversation,
  ConversationView as View,
  Message,
  OrchestratorStep,
  Question,
  QueuedMessage,
  Request,
  Settings,
  ThinkingSegment,
  UserRequest,
} from "@/ipc/generated";
import { loadConversation, openConversation } from "@/state/actions";
import { type Board, useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const drive = query.get("drive") ?? "turn";
const ID = "work-blocks";
const PEER = "work-blocks-peer";
const session = query.get("kind") === "session";
const begun = Date.now() - 30_000;

function conversation(id: string, title: string, kind: "chat" | "session"): Conversation {
  return {
    id, kind, projectId: null, title, pinnedAtMs: null, createdAtMs: begun, updatedAtMs: begun, lifecycle: "active",
    forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
    setup: kind === "chat"
      ? { type: "chat", model: { provider: "codex", model: null, effort: null, fast: false } }
      : {
          type: "session", repo: "/tmp/work-blocks-fixture", environment: { type: "local" }, permission: "fullAccess", planMode: false,
          workersSeeUncommitted: null, orchestrator: { provider: "codex", model: null, effort: null },
        },
  } as unknown as Conversation;
}

const conversations: Record<string, Conversation> = {
  [ID]: conversation(ID, "Fix the rounding", session ? "session" : "chat"),
  [PEER]: conversation(PEER, "Billing", "session"),
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

/** What the daemon holds now: the view a load reads. */
const daemon: { messages: Message[]; requests: Record<string, UserRequest>; steps: OrchestratorStep[]; run: "running" | "idle" } = {
  messages: [user("r1", 1, "Fix the rounding in math.js", begun)],
  requests: { r1: asked("r1", begun) },
  steps: [tool("read", 1.1, "Read", "/repo/src/math.js", "completed"), tool("test", 1.2, "shell", "npm test", "completed")],
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
    notices: [], memories: [], overnight: [], reviews: [], previews: [],
  } as unknown as View;
}

let settings = { followUp: "queue" } as Settings;
const calls: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return { method: req.method, view: view() };
    case "updateSettings":
      settings = req.settings;
      return { method: req.method, settings };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
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
  pinnedSummary: drive === "background",
  settings: { ...state.settings, followUp: "queue" },
}));
openConversation(ID);

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
createRoot(document.getElementById("root")!).render(<Page />);

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
    if (animating() || one("[data-slot=reply-loading]") || now !== last) stillSince = performance.now();
    else if (performance.now() - stillSince >= 100) return;
    last = now;
  }
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
    groupLine: text(one("[data-slot=group-live-line]", block ?? document)),
    groupTone: one("[data-slot=group-live-line]", block ?? document)?.getAttribute("data-tone") ?? null,
    shimmer: !!group?.querySelector(".shimmer"),
    status: text(status),
    thinkingLive: all("[data-slot=thinking-live]", block ?? document).length,
    commentary: text(one("[data-slot=commentary-live]", block ?? document)),
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

async function driveTurn(): Promise<object> {
  const out: Record<string, unknown> = {};
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
  change(() => ({ questions: {}, thinking: [thought(true)] }));
  steps([...daemon.steps, tool("sleep", 1.4, "shell", "sleep 4", "inProgress")]);
  await settle();
  out.running = seen();
  await hold("running");
  steps(daemon.steps.map((step) => (step.kind.type === "tool" && step.kind.itemId === "sleep" ? tool("sleep", 1.4, "shell", "sleep 4", "completed") : step)));
  change(() => ({ streaming: { messageId: "line", text: "I’ll fix the rounding next.\n\nThen run the tests again.", requestId: "r1", phase: "commentary", startedAtMs: Date.now() } }));
  await settle();
  out.commentary = seen();
  // A peer reply stays outstanding while the final answer streams.
  request("r1", { waitingFor: [PEER] });
  // The final answer starts: the work folds at once, the time stops there. The fold is timed from
  // the first frame that shows the answer starting, by the frames' own clock: a slow machine
  // takes its time drawing that frame, which is not the fold's.
  const answerAtMs = Date.now() - 1000;
  const answerText = "Fixed the rounding: `round` now rounds half-up, and the tests pass.";
  change(() => ({ streaming: { messageId: "a1", text: answerText.slice(0, 20), requestId: "r1", phase: "answer", startedAtMs: answerAtMs } }));
  await frame();
  const folding = Number(document.timeline.currentTime);
  out.foldFirst = one("[data-slot=request-fold]", turn() ?? document)?.getAttribute("data-fold") ?? "gone";
  let foldMs = -1;
  for (const end = performance.now() + 2000; performance.now() < end; ) {
    if (!one("[data-slot=request-fold]", turn() ?? document)) {
      foldMs = Math.round(Number(document.timeline.currentTime) - folding);
      break;
    }
    await frame();
  }
  out.foldMs = foldMs;
  await settle();
  out.answering = seen();
  // Ticks later it still says the same time.
  await wait(2200);
  change(() => ({ streaming: { messageId: "a1", text: answerText, requestId: "r1", phase: "answer", startedAtMs: answerAtMs } }));
  await settle();
  out.answeringLater = seen();
  // A question for the user while it streams: that wins over the wait for the other session.
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
  // The turn ends waiting for another session's reply: that line stays, under the answer.
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null }));
  request("r1", { state: { type: "waiting" }, waitingFor: [PEER], worked: [{ fromMs: begun, toMs: Date.now() }] });
  await settle();
  out.waiting = seen();
  // The reply came: the request is done.
  request("r1", { state: { type: "done" }, endedAtMs: Date.now(), waitingFor: [] });
  await settle();
  out.done = seen();
  await hold("done");
  out.actions = all("[data-slot=answer-actions] button", turn() ?? document).map((button) => button.getAttribute("aria-label") ?? text(one(".sr-only", button)));
  // Read again from the daemon, as after a restart.
  await loadConversation(ID);
  await settle(300);
  out.reloaded = seen();
  return out;
}

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
  change(() => ({ queue: { items: [], paused: false } }));
  // The user steers a message in while the command runs.
  const steered = user("r2", 2, "Use banker's rounding instead", Date.now());
  daemon.messages = [...daemon.messages, steered];
  daemon.requests.r2 = asked("r2", Date.now(), { steeredInto: "r1" });
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change((board) => ({ requests: { ...board.requests, r2: daemon.requests.r2! }, head: "r2" }));
  await settle();
  const after = seen();
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
  return { before, after, bubble, deciding, steerHeld, steerRead: text(one("[data-slot=request-steer] [data-slot=held-note]")) };
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
  // The thread's check outlasted its call: it went to the background, and the thread said so.
  steps([...daemon.steps, tool("check", 1.3, "mcp__brigadier__run_check", "cargo test --workspace", "completed")]);
  const said: Message = {
    id: "a1", conversationId: ID, seq: 2, role: "assistant", blob: null, createdAtMs: Date.now(), attachments: [], mentions: [],
    text: "The full Rust tests run in the background, about 15 minutes; I'll answer when they finish.", model: null,
    requestId: "r1", parentId: "r1", answerAtMs: Date.now(),
  };
  daemon.messages = [...daemon.messages, said];
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  const run: BackgroundRun = {
    id: "run-1", conversationId: ID, requestId: "r1", tool: "run_check", command: "cargo test --workspace",
    startedAtMs: Date.now() - 6 * 60_000 - 5_000, endedAtMs: null, state: { type: "running" },
  };
  daemon.run = "idle";
  change(() => ({ run: "idle", runRequest: null, head: "a1", backgroundRuns: { [run.id]: run } }));
  await settle(300);
  const card = () => one("[data-slot=background-run]");
  const running = {
    ...seen(),
    said: all("[data-slot=request-work]", turn() ?? document).some((part) => text(part)?.includes("about 15 minutes")),
    row: text(card()),
    elapsed: text(one("[data-slot=background-run-elapsed]")),
  };
  await hold("background");
  one<HTMLButtonElement>("button", card() ?? document.createElement("div"))?.click();
  for (const end = Date.now() + 3000; Date.now() < end && calls.length === 0; ) await wait(25);
  // The result came and the thread answered it: the request is done, the run's row gone.
  const answer: Message = { ...said, id: "a2", seq: 3, text: "The full Rust tests pass: 812 tests, no failures.", createdAtMs: Date.now(), answerAtMs: Date.now() };
  daemon.messages = [...daemon.messages, answer];
  useApp.setState((state) => ({ threads: { ...state.threads, [ID]: { ...state.threads[ID]!, items: daemon.messages } } }));
  change(() => ({ head: "a2", backgroundRuns: { [run.id]: { ...run, endedAtMs: Date.now(), state: { type: "finished", status: "exit 0" } } } }));
  request("r1", { state: { type: "done" }, endedAtMs: Date.now(), worked: [{ fromMs: begun, toMs: Date.now() }] });
  await settle(300);
  return { running, calls, done: { ...seen(), row: text(card()) } };
}

void (drive === "steer" ? driveSteer() : drive === "queue" ? driveQueue() : drive === "background" ? driveBackground() : driveTurn())
  .catch((error: unknown) => ({ error: String(error) }))
  .then((result) => {
    const out = document.createElement("pre");
    out.id = "work-blocks-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
