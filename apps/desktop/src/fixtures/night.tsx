/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread and the
 * side panel of the first real overnight run (the night of 2026-10-03), from its stored events
 * (`boards/overnight-2026-10-03.json`). Requests are answered here and never reach a daemon.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Message, Request, Task } from "@/ipc/generated";
import { type Board, emptyBoard, useBoard } from "@/state/board";
import { emptyThread, useApp } from "@/state/store";

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
useBoard.setState({
  board: {
    ...emptyBoard(id),
    ...(night as unknown as Partial<Board>),
    tasks,
    loaded: true,
    head: messages.at(-1)?.id ?? null,
  } as Board,
});
useApp.setState({
  conversations: { [id]: conversation },
  threads: { [id]: { ...emptyThread, items: messages } },
  pinnedSummary: true,
});

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
    default:
      return { method: req.method };
  }
});

function NightPage() {
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1" defaultOpen={false}>
          <main className="relative flex h-full min-w-0 flex-1">
            <div className="bg-background relative flex h-full min-w-0 flex-1 flex-col">
              <ConversationView selection={{ type: "conversation", id }} />
            </div>
          </main>
        </SidebarProvider>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<NightPage />);
