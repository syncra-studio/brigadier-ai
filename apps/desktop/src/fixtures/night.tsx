/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread and the
 * side panel of the first real overnight run (the night of 2026-10-03), from its stored events
 * (`boards/overnight-2026-10-03.json`), in the app's own window layout. Requests are answered here
 * and never reach a daemon.
 *
 * Query: `density=compact`; `sidebar=0` (closed); `run=live` (the run still working) or
 * `run=none` (a normal session: its plan is an ordinary plan card); `plans=N` (N more plan cards).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { AppSidebar, TitlebarToggle } from "@/app/AppSidebar";
import { BottomBar } from "@/app/BottomBar";
import { ConversationView } from "@/app/ConversationView";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Message, Request, Task } from "@/ipc/generated";
import { revealOvernight, revealPlan, useSummary } from "@/app/conversation/summaryState";
import { type Board, emptyBoard, useBoard } from "@/state/board";
import { emptyThread, useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
document.documentElement.dataset.density = query.get("density") === "compact" ? "compact" : "normal";
const id = night.conversationId;
const created = Math.min(...night.messages.map((message) => (message as unknown as Message).createdAtMs));
const conversation: Conversation = {
  id,
  kind: "session",
  projectId: null,
  title: "/overnight Make overnight runs faster and leaner",
  pinnedAtMs: null,
  createdAtMs: created,
  updatedAtMs: created,
  lifecycle: "active",
  forkedFrom: null,
  sideOf: null,
  fallback: null,
  quotaWait: null,
  setup: {
    type: "session",
    repo: "/tmp/night-fixture",
    environment: { type: "newWorktree", branch: "brigadier/4158464b/session", base: "main" },
    permission: "approveForMe",
    planMode: false,
    workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
} as Conversation;

const messages = night.messages as unknown as Message[];
// The stored fixture keeps only what the thread groups by; the worker panel reads whole reports.
const tasks = Object.fromEntries(
  Object.entries(night.tasks as unknown as Record<string, Task>).map(([taskId, task]) => [
    taskId,
    task.report
      ? {
          ...task,
          report: Object.assign(
            {
              summary: "",
              changes: [],
              decisions: [],
              verification: [],
              doneWhen: [],
              openQuestions: [],
              risks: [],
              artifacts: [],
              submittedAtMs: task.updatedAtMs,
            },
            task.report,
          ),
        }
      : task,
  ]),
);
const stored = night as unknown as Board;
let overnight = stored.overnight;
let plans = stored.plans;
if (query.get("run") === "live") {
  overnight = Object.fromEntries(
    Object.entries(overnight).map(([runId, run]) => [
      runId,
      { ...run, state: "running", finishedAtMs: null, reportMessageId: null, reportOutcome: null, stop: null },
    ]),
  ) as Board["overnight"];
} else if (query.get("run") === "none") {
  overnight = {};
  // Without its run, the run's plan reads as a session's own plan.
  plans = Object.fromEntries(
    Object.entries(plans).map(([planId, plan]) => [planId, { ...plan, requestId: `request-${planId}` }]),
  );
}
const live = Object.values(plans).find((plan) => plan.state.type !== "superseded");
if (live) {
  for (let n = 1; n <= Number(query.get("plans") ?? 0); n++) {
    const planId = `${live.id}-more-${n}`;
    plans = { ...plans, [planId]: { ...live, id: planId, title: `${live.title} (${n + 1})`, requestId: `request-${planId}`, position: live.position + n } };
  }
}
useBoard.setState({
  board: {
    ...emptyBoard(id),
    ...stored,
    overnight,
    plans,
    tasks,
    loaded: true,
    head: messages.at(-1)?.id ?? null,
  } as Board,
});
useApp.setState({
  conversations: { [id]: conversation },
  threads: { [id]: { ...emptyThread, items: messages } },
  pinnedSummary: true,
  connection: { status: "connected", daemon: null, reason: null },
});

// For probes driving the page.
Object.assign(window, { night: { useApp, useBoard, useSummary, revealPlan, revealOvernight } });

const calls: unknown[] = [];
(window as typeof window & { nightCalls: unknown[] }).nightCalls = calls;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    case "getRunDiff":
      return { method: req.method, diff: null };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
});

/** The app's window layout (App.tsx) around the session, without its dialogs. */
function NightPage() {
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

createRoot(document.getElementById("root")!).render(<NightPage />);
