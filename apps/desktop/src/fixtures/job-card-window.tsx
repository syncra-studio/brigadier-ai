/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's whole
 * window while a worker builds, CI checks a push, and the thread asks the user something (IPC
 * mocked). A plain session shows no Job card: the question card stays above the composer (R3),
 * CI is a Background row with Open CI, and the rail above the composer holds the Workers strip
 * alone. What it saw goes as
 * JSON into `<pre id="job-card-window-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Conversation, ConversationView as View, OrchestratorStep, Question, Request, Task, UserRequest } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const id = "job-card-window";
const start = Date.now() - 20 * 60_000;
const session = {
  id, kind: "session", projectId: null, title: "Shared MCP and skills", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "brigadier/s" }, permission: "ask", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } },
} as unknown as Conversation;

const base = Object.values(night.tasks)[0] as unknown as Task;
const worker: Task = { ...base, id: "t1", conversationId: id, title: "Shared MCP and skills", kind: "implement", state: "running", requestId: "r1", subject: null, createdAtMs: start + 60_000, updatedAtMs: start + 60_000 } as Task;
const request = { id: "r1", conversationId: id, preview: "Share MCP servers", state: { type: "waiting" }, startedAtMs: start, endedAtMs: null, steeredInto: null, steeredAfter: null, undo: null, worked: [] } as unknown as UserRequest;
const question: Question = {
  id: "q1", conversationId: id, taskId: null, requestId: "r1", position: 0, kind: { type: "orchestrator" },
  text: "Build the Plugins and Skills page as drawn?", options: ["Yes", "Change it"], recommended: 0,
  items: [{ text: "Build the Plugins and Skills page as drawn?", options: [{ label: "Yes", description: null }, { label: "Change it", description: null }], recommended: 0 }],
  answer: null, answers: [], createdAtMs: start + 5 * 60_000, answeredAtMs: null,
};
const step = (atMs: number, kind: unknown): OrchestratorStep => ({ requestId: "r1", kind: kind as OrchestratorStep["kind"], atMs, position: 0 });
const steps = [
  step(start + 3 * 60_000, {
    type: "ciWatch", repo: "o/r", branch: "main", sha: "4e1c49e99aa", waiting: false,
    runs: [{ workflow: "test", url: "https://github.com/o/r/actions/runs/1", status: "in_progress", conclusion: "", jobsDone: 3, jobsTotal: 5 }],
  }),
];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation: session, context: null,
          messages: { messages: [], hasMore: false }, tasks: [worker], approvals: [], questions: [question],
          plans: [], requests: [request], workerSteps: [], orchestratorSteps: steps, machineSteps: [], thinking: [],
          compactions: [], decisions: [], waiting: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [], memories: [], overnight: [],
          previews: [],
          // The thread's headline: outside a plan or an overnight run no card shows it.
          headline: { text: "Building the registry", detail: "Next: the Plugins and Skills page", failed: false, atMs: start + 4 * 60_000 },
        } as unknown as View,
      };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: session },
  summaryShown: { [id]: true },
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { status: "connected", daemon: null, reason: null },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}

const row = (activity: string) => document.querySelector<HTMLElement>(`[data-activities] [data-activity="${activity}"]`);
const composer = () => document.querySelector("[data-slot=composer]");

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the Background rows", () => row("ci") !== null);
    await until("the question card", () => composer()?.textContent?.includes("Build the Plugins and Skills page") ?? false);
    await until("the Workers strip", () => composer()?.querySelector("[data-slot=composer-status-slot]") !== null);
    await sleep(200);
    seen.jobCards = document.querySelectorAll("[data-slot=job-card]").length;
    const buttons = (activity: string) => [...(row(activity)?.parentElement?.querySelectorAll('[data-slot="summary-row-action"] button') ?? [])].map((button) => button.textContent);
    seen.background = { ci: buttons("ci") };
    const rail = composer()!.querySelector("[data-slot=composer-status-slot]")!;
    seen.rail = rail.textContent;
    seen.railRows = rail.children.length;
    seen.railHasCi = /CI/.test(composer()!.textContent ?? "");
    // The question card sits above the composer's field.
    const field = composer()!.querySelector("[data-slot=composer-input]");
    const asking = [...composer()!.querySelectorAll("*")].find((node) => node.children.length === 0 && node.textContent?.includes("Build the Plugins and Skills page"));
    seen.questionAboveField = !!asking && !!field && (asking.compareDocumentPosition(field) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "job-card-window-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
