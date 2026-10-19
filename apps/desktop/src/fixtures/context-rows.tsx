/**
 * The production context card of a worktree session with something in every section, in the
 * reference order: the project with its ⋯, its Changes and Reviewed rows, the plan, the
 * workers (two at work, one done), what runs in the background and the sources. For screenshots
 * and render checks; synthetic IPC, no daemon. `?workers=5` puts five at work (one row).
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { AgentsPanelContext, type AgentsPanelState } from "@/app/conversation/WorkerChip";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { BackgroundRun, Conversation, IntegratedCheck, OrchestratorStep, Plan, Preview, Request, ReviewRun, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "context-rows-fixture";
const params = new URLSearchParams(location.search);
const now = Date.now();
const MIN = 60_000;
const conversation: Conversation = { id, kind: "session", title: "Add the web interface", projectId: "p1",
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "session", repo: "/work/todo-app", environment: { type: "newWorktree", branch: "brigadier/web-ui", base: "main", path: "/work/todo-app-web-ui", start: null },
    permission: "askForApproval", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } } };
const base = Object.values(night.tasks)[0] as unknown as Task;
const worker = (taskId: string, number: number, title: string, state: Task["state"], minutes: number): Task => ({ ...base, id: taskId, number, title, conversationId: id, state,
  kind: "implement", role: null, phase: null, run: null, attempts: [], requestId: "r1", gateLink: null, candidate: null, report: null, landed: state === "landed",
  quotaWait: null, blockedReason: null, review: null, createdAtMs: now - minutes * MIN, updatedAtMs: now }) as unknown as Task;
const working = Number(params.get("workers") ?? 2);
const names = ["Web page", "API routes", "Styles", "Tests", "Readme"];
const tasks = Object.fromEntries([
  ...names.slice(0, working).map((name, index) => worker(`task-${index + 2}`, index + 2, name, "running", 6 - index)),
  worker("task-1", 1, "Data model", "landed", 20),
].map((task) => [task.id, task]));
const plan = { ...(Object.values(night.plans)[0] as unknown as Plan), id: "plan-1", conversationId: id, requestId: "r1", position: 1, title: "Add a web interface to the todo app",
  body: "# Add a web interface\n\nA page that lists, adds and completes todos.", state: { type: "approved", by: "user" } } as Plan;
const { settled: _settled, ...open } = plan.steps[0]!;
plan.steps = [
  { ...plan.steps[0]!, title: "Data model and storage", taskId: "task-1", number: 1 },
  { ...open, title: "API routes", taskId: "task-3", number: 2, endedAtMs: null },
  { ...open, title: "The web page", taskId: "task-2", number: 3, endedAtMs: null },
];
const review: ReviewRun = { id: "review-1", conversationId: id, requestId: "r1", taskId: null, kind: "code", base: "a1", tip: "b2", author: "claude", reviewer: "codex",
  reviewerModel: "reviewer", notify: { type: "orchestrator" }, state: { type: "findings", count: 5 }, startedAtMs: now - 9 * MIN, endedAtMs: now - 7 * MIN, findings: null, number: 1,
  items: [1, 2, 3, 4, 5].map((n) => ({ id: `R1.${n}`, severity: "P2", title: `Finding ${n}`, location: "src/server.js:12", high: false,
    status: { type: "fixed", task: null, why: null, checked: "its test passes at the tip" } })) } as ReviewRun;
const checks: IntegratedCheck = { id: "check-1", conversationId: id, repo: "/work/todo-app", branch: "brigadier/web-ui", tip: "b2c3d4e5f6a7",
  dueAtMs: now - 5 * MIN, startedAtMs: now - 5 * MIN, state: { type: "passed", commands: ["npm test"] } } as IntegratedCheck;
const page: Preview = { id: "preview-1", conversationId: id, name: "todo app", command: "npm run dev", kind: "web", workdir: "/work/todo-app-web-ui",
  workspace: "/work/todo-app-web-ui", pid: 42, log: null, url: "http://localhost:3000/", state: { type: "running" }, building: false, startedAtMs: now - 3 * MIN, endedAtMs: null };
const run: BackgroundRun = { id: "run-1", conversationId: id, requestId: "r1", tool: "run_check", name: "api tests", command: "npm test -- --watch",
  startedAtMs: now - 2 * MIN, endedAtMs: null, state: { type: "running" } };
const step = (kind: OrchestratorStep["kind"], at: number): OrchestratorStep => ({ requestId: "r1", kind, atMs: now - at * MIN, position: 0 });
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, tasks, plans: { [plan.id]: plan }, reviews: { [review.id]: review }, integratedChecks: { [checks.id]: checks },
  previews: { [page.id]: page }, backgroundRuns: { [run.id]: run },
  diffs: { "task-2": { files: [], insertions: 120, deletions: 4 }, "task-3": { files: [], insertions: 48, deletions: 0 } },
  orchestratorSteps: [
    step({ type: "readPage", url: "https://expressjs.com/en/starter/static-files.html" } as OrchestratorStep["kind"], 12),
    step({ type: "readPage", url: "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/form" } as OrchestratorStep["kind"], 11),
  ] } });
useApp.setState({
  summaryShown: { [id]: true }, selection: { type: "conversation", id }, conversations: { [id]: conversation },
  projects: { p1: { id: "p1", name: "todo-app" } } as never,
  info: { platform: "macos", version: "fixture", processStartMs: now, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "getSessionDiff") return { method: req.method, stat: { files: [], insertions: 380, deletions: 2 }, commits: 4 };
  if (req.method === "getWorkerDiffs") return { method: req.method, diffs: useBoard.getState().board?.diffs ?? {} };
  return { method: req.method };
});

const card = () => document.querySelector<HTMLElement>('[data-slot="summary-card"]');
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 50)));

/** What the card shows at rest, and what its clicks do: they open things, never grow the card. */
async function drive(panel: () => AgentsPanelState): Promise<string> {
  for (let i = 0; i < 100 && !card()?.querySelector('[data-slot="workers-summary-worker"], [data-slot="workers-summary"]'); i++) await frame();
  await frame();
  const seen: Record<string, unknown> = {};
  seen.sections = [...card()!.querySelectorAll('[data-slot="summary-section"] > header button[aria-controls]')].map((title) => title.textContent?.trim());
  seen.projectMenu = !!card()!.querySelector('[data-slot="summary-section"] > header button[aria-label="Project actions"], [data-slot="summary-section"] > header button[aria-haspopup]');
  const rows = [...card()!.querySelectorAll<HTMLElement>('[data-slot="summary-row"], [data-slot="workers-summary"], [data-slot="workers-summary-worker"]')].filter((row) => !row.closest("[data-activities]"));
  seen.rowHeights = [...new Set(rows.map((row) => Math.round(row.getBoundingClientRect().height)))];
  seen.odd = rows.filter((row) => Math.round(row.getBoundingClientRect().height) !== 28).map((row) => row.outerHTML.slice(0, 300));
  seen.workers = [...card()!.querySelectorAll('[aria-label="Session workers"] [data-slot^="workers-summary"]')].map((row) => row.textContent);
  const height = card()!.getBoundingClientRect().height;
  const planStep = [...card()!.querySelectorAll<HTMLButtonElement>('[data-card="plan"] li button')].find((button) => button.textContent?.includes("API routes"));
  planStep?.click();
  await frame();
  seen.stepOpens = panel();
  const project = card()!.querySelector('[data-slot="summary-section"]')!;
  seen.project = [...project.querySelectorAll('[data-slot="summary-row"]')].map((row) => row.textContent?.replace(/\s+/g, " ").trim());
  seen.changesTip = project.querySelector('[data-row="changes"]')?.getAttribute("title");
  seen.findingsInCard = card()!.querySelectorAll('[data-slot*="finding"]').length;
  project.querySelector<HTMLButtonElement>('[data-row="reviewed"]')?.click();
  await frame();
  seen.reviewedOpens = sessionTabs(id).active;
  project.querySelector<HTMLButtonElement>('[data-row="changes"]')?.click();
  await frame();
  const front = sessionTabs(id).tabs.find((tab) => tab.id === sessionTabs(id).active);
  seen.changesOpens = front?.kind === "review" ? front.target.type : front?.kind;
  seen.grew = card()!.getBoundingClientRect().height !== height;
  return JSON.stringify(seen);
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [panel, setPanel] = useState<AgentsPanelState>(undefined);
  const [result, setResult] = useState("");
  const shown = useRef<AgentsPanelState>(undefined);
  useEffect(() => {
    shown.current = panel;
  }, [panel]);
  useEffect(() => {
    if (params.has("drive")) void drive(() => shown.current).then(setResult);
  }, []);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <AgentsPanelContext.Provider value={{ panel, setPanel }}>
      <main className="bg-background text-foreground relative flex h-screen w-full min-w-0">
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="h-titlebar flex items-center px-5 text-sm font-medium">Add the web interface</header>
          <PinnedSummary conversation={conversation} />
        </div>
      </main>
      </AgentsPanelContext.Provider>
      {result && <pre id="context-rows-result">{result}</pre>}
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
