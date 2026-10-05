/**
 * A standalone Vite entry, excluded from the app entry and production build: a session run the
 * phase way (a lead per phase, the orchestrator's answers, landings), in the app's own window
 * layout, from synthetic data. Requests are answered here and never reach a daemon.
 *
 * Query: `view=running` (default: the second request works, three approvals wait) or
 * `view=done` (it finished, with a final answer and what waits on the user); `approvals=0`
 * (none wait); `worker=1` (the lead's own thread open in the Workers panel); `sidebar=0`;
 * `density=compact`. `window.flow` holds the stores for probes.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { AppSidebar, TitlebarToggle } from "@/app/AppSidebar";
import { BottomBar } from "@/app/BottomBar";
import { ConversationView } from "@/app/ConversationView";
import { revealPlan, useSummary } from "@/app/conversation/summaryState";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type {
  Approval,
  ApprovalSubject,
  Conversation,
  Message,
  OrchestratorStep,
  OrchestratorStepKind,
  Plan,
  ProviderEvent,
  RawEntry,
  Request,
  Task,
  UserRequest,
  WaitingItem,
} from "@/ipc/generated";
import { useActivity } from "@/state/activity";
import { type Board, emptyBoard, useBoard } from "@/state/board";
import { emptyThread, useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
document.documentElement.dataset.density = query.get("density") === "compact" ? "compact" : "normal";
const done = query.get("view") === "done";
const approvals = !done && query.get("approvals") !== "0";

const id = "flow-session";
const NOW = Date.now();
const ago = (seconds: number) => NOW - seconds * 1000;
const BRANCH = "brigadier/flow/session";

const conversation = {
  id,
  kind: "session",
  projectId: null,
  title: "Dark mode toggle in Settings",
  pinnedAtMs: null,
  createdAtMs: ago(2400),
  updatedAtMs: ago(10),
  lifecycle: "active",
  forkedFrom: null,
  sideOf: null,
  fallback: null,
  quotaWait: null,
  setup: {
    type: "session",
    repo: "/tmp/flow-fixture",
    environment: { type: "newWorktree", branch: BRANCH, base: "main" },
    permission: "askForApproval",
    planMode: false,
    workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
} as Conversation;

function message(seq: number, role: "user" | "assistant", requestId: string, text: string, at: number): Message {
  return {
    id: `m${seq}`,
    conversationId: id,
    seq,
    role,
    text,
    blob: null,
    createdAtMs: at,
    attachments: [],
    mentions: [],
    model: null,
    requestId,
    parentId: null,
  } as unknown as Message;
}

const messages: Message[] = [
  message(1, "user", "r1", "Fix the typo in the README's install section.", ago(2400)),
  message(
    8,
    "assistant",
    "r1",
    "Fixed the typo in `README.md` (“pnpm instal” → “pnpm install”) and landed it on `brigadier/flow/session`. `pnpm lint` passes.",
    ago(2208),
  ),
  message(10, "user", "r2", "Add a dark mode toggle to Settings, remember the choice, and cover it with tests.", ago(done ? 1260 : 134)),
  message(11, "assistant", "r2", "I’ll have a lead read the settings code and outline the change, then build it in two phases.", ago(done ? 1255 : 128)),
  message(
    15,
    "assistant",
    "r2",
    "The outline holds up. One correction for the lead: keep the choice in the existing settings store rather than a new file.",
    ago(done ? 1180 : 70),
  ),
];
if (done) {
  messages.push(
    message(
      30,
      "assistant",
      "r2",
      [
        "Settings → Appearance now has a **Theme** switch: System, Light or Dark. The choice is kept in the settings store and applied before the first paint, so the window never flashes the wrong theme.",
        "",
        "- Phase 1 added the switch and the theme setting (2 commits).",
        "- Phase 2 kept the choice across restarts and added 6 tests (1 commit).",
        "- The verifier ran `pnpm typecheck`, `pnpm lint`, `pnpm test` and `pnpm build`: all pass.",
      ].join("\n"),
      ago(15),
    ),
  );
}

const requests: Record<string, UserRequest> = {
  r1: { id: "r1", conversationId: id, preview: "Fix the typo", state: { type: "done" }, startedAtMs: ago(2400), endedAtMs: ago(2208), steeredInto: null, steeredAfter: null, undo: null } as UserRequest,
  r2: {
    id: "r2",
    conversationId: id,
    preview: "Add a dark mode toggle",
    state: { type: done ? "done" : "working" },
    startedAtMs: ago(done ? 1260 : 134),
    endedAtMs: done ? ago(15) : null,
    steeredInto: null,
    steeredAfter: null,
    undo: null,
  } as UserRequest,
};

// A real task record of the fixture night, as the shape every task here starts from.
const base = Object.values(night.tasks)[0] as unknown as Task;
const LEAD_SPEC = [
  "Add a Theme switch to Settings → Appearance with System, Light and Dark.",
  "",
  "Code pointers: apps/desktop/src/app/settings/Appearance.tsx (the section), state/settings.ts (the store).",
  "",
  "Done when:",
  "1. The switch changes the theme at once.",
  "2. The choice survives a restart.",
  "3. pnpm typecheck, lint, test and build pass.",
].join("\n");

function task(fields: Partial<Task> & Pick<Task, "id" | "number" | "title" | "state" | "requestId" | "position">): Task {
  return {
    ...base,
    conversationId: id,
    kind: "implement",
    spec: LEAD_SPEC,
    route: { choice: { provider: "claude", model: "opus", effort: "high" }, reason: "", explanation: null },
    attempts: [],
    review: null,
    gate: null,
    gateLink: null,
    candidate: null,
    fixRounds: 0,
    fixes: [],
    landed: null,
    blockedReason: null,
    error: null,
    run: null,
    report: null,
    role: "lead",
    phase: null,
    quotaWait: null,
    subject: null,
    messages: [],
    createdAtMs: ago(60),
    updatedAtMs: ago(10),
    ...fields,
  } as Task;
}

const tasks: Record<string, Task> = {
  t1: task({ id: "t1", number: 1, title: "Fix the README typo", state: "landed", requestId: "r1", position: 3, spec: "Fix the typo “pnpm instal” in README.md’s install section.", createdAtMs: ago(2390), updatedAtMs: ago(2215), phase: null }),
  t2: task({
    id: "t2",
    number: 2,
    title: "Theme setting and switch",
    state: done ? "landed" : "running",
    requestId: "r2",
    position: 12,
    phase: 1,
    createdAtMs: ago(done ? 1250 : 115),
    updatedAtMs: done ? ago(700) : ago(2),
  }),
};
if (done) {
  tasks["t3"] = task({ id: "t3", number: 3, title: "Verify phase 1", role: "verifier", phase: 1, state: "done", requestId: "r2", position: 20, kind: "verify", spec: "Check phase 1 against its done-when and review its diff.", createdAtMs: ago(690), updatedAtMs: ago(520) });
  tasks["t4"] = task({ id: "t4", number: 4, title: "Keep the theme and test it", phase: 2, state: "landed", requestId: "r2", position: 24, spec: "Keep the theme choice across restarts and add tests for the switch.", createdAtMs: ago(500), updatedAtMs: ago(60) });
}

function step(position: number, requestId: string, kind: OrchestratorStepKind, at: number): OrchestratorStep {
  return { requestId, kind, atMs: at, position } as OrchestratorStep;
}

const orchestratorSteps: OrchestratorStep[] = [
  step(2, "r1", { type: "created", taskId: "t1" }, ago(2390)),
  step(7, "r1", { type: "landed", taskIds: ["t1"], commits: 1, branch: BRANCH, head: "a1b2c3d" }, ago(2210)),
  step(12, "r2", { type: "created", taskId: "t2" }, ago(done ? 1250 : 115)),
  step(
    16,
    "r2",
    {
      type: "answered",
      taskId: "t2",
      question: "Should the switch follow the system setting by default?",
      answer: "Yes: default to System, with Light and Dark as choices",
      why: "the app already follows the system theme everywhere else",
    },
    ago(done ? 1100 : 40),
  ),
];
if (done) {
  orchestratorSteps.push(
    step(19, "r2", { type: "created", taskId: "t3" }, ago(690)),
    step(22, "r2", { type: "landed", taskIds: ["t2"], commits: 2, branch: BRANCH, head: "d4e5f6a" }, ago(510)),
    step(23, "r2", { type: "created", taskId: "t4" }, ago(500)),
    step(27, "r2", { type: "landed", taskIds: ["t4"], commits: 1, branch: BRANCH, head: "b7c8d9e" }, ago(50)),
  );
}

const plan = {
  id: "p1",
  conversationId: id,
  requestId: "r2",
  position: 13,
  title: "Dark mode toggle",
  risky: false,
  state: { type: "approved", by: "orchestrator" },
  reviewSkipReason: null,
  gate: null,
  revises: null,
  responses: [],
  reviewNotes: [],
  createdAtMs: ago(done ? 1245 : 110),
  decidedAtMs: ago(done ? 1180 : 70),
  steps: [
    { title: "Theme setting and switch", detail: "The setting in the store and the switch in Appearance.", taskId: "t2", stage: done ? "done" : "building", startedAtMs: ago(done ? 1250 : 115), endedAtMs: done ? ago(510) : null, outline: null },
    { title: "Keep the theme and test it", detail: "Keep the choice across restarts; tests for the switch.", taskId: done ? "t4" : null, stage: done ? "done" : "pending", startedAtMs: done ? ago(500) : null, endedAtMs: done ? ago(50) : null, outline: null },
  ],
} as unknown as Plan;

const GATE_CURL =
  "/Users/me/Library/Application Support/Brigadier/gate/bin/git -c core.hooksPath=/dev/null -c credential.helper= ls-remote https://github.com/radix-ui/primitives";

function approval(n: number, subject: ApprovalSubject, taskId: string | null): Approval {
  return {
    id: `a${n}`,
    conversationId: id,
    taskId,
    requestId: "r2",
    position: 16 + n,
    subject,
    state: { type: "pending" },
    createdAtMs: ago(30 - n),
    resolvedAtMs: null,
  } as Approval;
}

const pending: Record<string, Approval> = approvals
  ? {
      a1: approval(
        1,
        {
          type: "cli",
          request: {
            id: "q1",
            kind: "command",
            tool: "Bash",
            command: `/bin/zsh -lc '${GATE_CURL}'`,
            cwd: "/tmp/flow-fixture",
            paths: [],
            reason: "May I check the switch component’s published versions on GitHub?",
            escalation: true,
            input: null,
            grant: "git ls-remote",
          },
        },
        "t2",
      ),
      a2: approval(
        2,
        { type: "action", action: "Use the signing key in your keychain to sign the test build?", details: "Needed once, to check that a signed build still starts in Dark mode." },
        "t2",
      ),
      a3: approval(
        3,
        {
          type: "outline",
          taskId: "t2",
          title: "Dark mode toggle",
          outline: "1. Add `theme: \"system\" | \"light\" | \"dark\"` to the settings store.\n2. A Theme switch in Settings → Appearance.\n3. Apply it before the first paint.\n4. Tests for the store and the switch.",
        },
        "t2",
      ),
    }
  : {};

const waiting: Record<string, WaitingItem> = done
  ? {
      w1: { id: "w1", requestId: "r2", source: { type: "orchestrator" }, key: "push", what: `Push ${BRANCH} when you’re happy with it`, createdAtMs: ago(20) } as WaitingItem,
      w2: { id: "w2", requestId: "r2", source: { type: "orchestrator" }, key: "icon", what: "Pick an icon for the Theme row (two options are in the report)", createdAtMs: ago(19) } as WaitingItem,
    }
  : {};

let seq = 0;
function entry(event: ProviderEvent, at: number): RawEntry {
  seq += 1;
  return { streamSeq: seq, atMs: at, event };
}

/** The lead's own transcript, for its thread in the Workers panel. */
export const leadTranscript: RawEntry[] = [
  entry({ type: "message", itemId: "x1", role: "assistant", text: "I’ll read the settings code first, then outline the change." }, ago(110)),
  entry({ type: "toolCall", itemId: "x2", name: "Read", input: JSON.stringify({ file_path: "apps/desktop/src/app/settings/Appearance.tsx" }), status: "completed", output: "…" }, ago(108)),
  entry({ type: "command", itemId: "x3", command: "/bin/zsh -lc 'rg -n theme apps/desktop/src/state'", cwd: "/tmp/flow-fixture", status: "completed", exitCode: 0, output: "state/settings.ts:41:  theme?: never;\nstate/settings.ts:88:  // theme follows the system", durationMs: 120 }, ago(106)),
  entry(
    {
      type: "toolCall",
      itemId: "x4",
      name: "WebSearch",
      input: JSON.stringify({ query: "radix switch prefers-color-scheme react 19" }),
      status: "completed",
      output:
        'Web search results for query: "radix switch prefers-color-scheme react 19"\n\nLinks: [{"title":"Switch – Radix Primitives","url":"https://www.radix-ui.com/primitives/docs/components/switch"},{"title":"prefers-color-scheme - CSS | MDN","url":"https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-color-scheme"},{"title":"Dark mode in React without a flash","url":"https://joshwcomeau.com/react/dark-mode/"}]',
    },
    ago(100),
  ),
  entry({ type: "toolCall", itemId: "x4b", name: "WebSearch", input: JSON.stringify({ query: "radix switch changelog 2026" }), status: "failed", output: "Web search failed: the request timed out after 30 seconds." }, ago(95)),
  entry({ type: "message", itemId: "x5", role: "assistant", text: "The store has no theme yet. I’ll add it, then the switch." }, ago(90)),
  entry({ type: "fileChanges", itemId: "x6", changes: [{ path: "apps/desktop/src/state/settings.ts", kind: "update" }, { path: "apps/desktop/src/app/settings/Appearance.tsx", kind: "update" }], status: "completed" }, ago(60)),
  entry({ type: "command", itemId: "x7", command: "/bin/zsh -lc 'pnpm test settings'", cwd: "/tmp/flow-fixture", status: "failed", exitCode: 1, output: "✖ the switch applies the theme at once\n  AssertionError: expected 'dark' to equal 'light'\nℹ pass 11\nℹ fail 1", durationMs: 4200 }, ago(40)),
  entry(
    {
      type: "error",
      error: {
        kind: "overloaded",
        message:
          "The model is overloaded right now (529). The request was retried 3 times over 40 seconds and still failed; the CLI will try again shortly. If this keeps happening, Brigadier moves the worker to another model of the same tier.",
        willRetry: true,
        limit: null,
        code: "overloaded_error",
      },
    },
    ago(30),
  ),
  entry({ type: "command", itemId: "x8", command: "/bin/zsh -lc 'pnpm test settings'", cwd: "/tmp/flow-fixture", status: "inProgress", exitCode: null, output: null, durationMs: null }, ago(3)),
];

useBoard.setState({
  board: {
    ...emptyBoard(id),
    tasks,
    plans: { p1: plan },
    requests,
    orchestratorSteps,
    approvals: pending,
    waiting,
    activity: done ? {} : { t2: "Running pnpm test settings" },
    transcripts: { t2: { entries: leadTranscript, hasMore: false, loading: false } },
    loaded: true,
    head: messages.at(-1)?.id ?? null,
  } as Board,
});
useApp.setState({
  selection: { type: "conversation", id },
  conversations: { [id]: conversation },
  threads: { [id]: { ...emptyThread, items: messages } },
  pinnedSummary: query.get("summary") !== "0",
  connection: { status: "connected", daemon: null, reason: null },
});

// The sidebar row's spinner or "Awaiting approval" pill.
useActivity.setState({
  byConversation: {
    [id]: {
      run: done ? "idle" : "running",
      tasks: Object.fromEntries(Object.values(tasks).map((each) => [each.id, each.state])),
      approvals: new Set(Object.keys(pending)),
      questions: new Set(),
    },
  },
});

const calls: unknown[] = [];
Object.assign(window, { flow: { useApp, useBoard, useSummary, revealPlan, calls } });
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "answerCard": {
      // The daemon settles the card; here it simply goes.
      const { cardId: approvalId } = req;
      useBoard.setState((s) => {
        if (!s.board) return s;
        const { [approvalId]: _answered, ...rest } = s.board.approvals;
        return { board: { ...s.board, approvals: rest } };
      });
      useActivity.setState((s) => {
        const activity = s.byConversation[id];
        if (!activity) return s;
        const left = new Set(activity.approvals);
        left.delete(approvalId);
        return { byConversation: { ...s.byConversation, [id]: { ...activity, approvals: left } } };
      });
      return { method: req.method };
    }
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    case "getRunDiff":
      return { method: req.method, diff: null };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    case "listWorkerEvents":
      return { method: req.method, page: { entries: leadTranscript, hasMore: false } };
    default:
      return { method: req.method };
  }
});

/** The app's window layout (App.tsx) around the session, without its dialogs. */
function FlowPage() {
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen={query.get("sidebar") !== "0"}>
          <div
            aria-hidden
            data-slot="page-surface"
            className="bg-background rounded-page shadow-page top-titlebar start-surface-inset end-surface-inset bottom-bottom-bar pointer-events-none absolute"
          />
          <div className="relative flex min-h-0 min-w-0 flex-1 ps-surface-inset pe-surface-inset">
            <SidebarPanel>
              <AppSidebar />
            </SidebarPanel>
            <main className="body-divider relative flex h-full min-w-0 flex-1">
              <div className="relative flex h-full min-w-0 flex-1 flex-col">
                <ConversationView selection={{ type: "conversation", id }} />
              </div>
            </main>
          </div>
          <BottomBar />
          <TitlebarToggle />
        </SidebarProvider>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<FlowPage />);
