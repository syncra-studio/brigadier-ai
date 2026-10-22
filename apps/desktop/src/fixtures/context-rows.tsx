/**
 * The production context card of a worktree session with something in every section, in the
 * reference order: the project and its branch row, the workers (two at work, one done), what
 * runs in the background and the sources; under it, the Job card of the plan's phase at work
 * (Q88). For screenshots and render checks; synthetic IPC, no daemon. `?workers=5` puts five at work (one row).
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { SessionJobTab } from "@/app/side-panel/JobTab";
import { SourcesTab } from "@/app/side-panel/SourcesTab";
import { AgentsPanelContext, type AgentsPanelState } from "@/app/conversation/WorkerChip";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { BackgroundRun, Conversation, OrchestratorStep, Plan, Preview, Request, Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { openPanelTab, sessionTabs, useSessionTabsOf } from "@/state/sessionTabs";
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
  // Phase 2 began after the first phase.
  { ...open, title: "API routes", taskId: "task-3", number: 2, startedAtMs: now - 4.5 * MIN, endedAtMs: null },
  { ...open, title: "The web page", taskId: "task-2", number: 3, endedAtMs: null },
];
const page: Preview = { id: "preview-1", conversationId: id, name: "todo app", command: "npm run dev", kind: "web", workdir: "/work/todo-app-web-ui",
  workspace: "/work/todo-app-web-ui", pid: 42, log: null, url: "http://localhost:3000/", state: { type: "running" }, building: false, startedAtMs: now - 3 * MIN, endedAtMs: null };
const run: BackgroundRun = { id: "run-1", conversationId: id, requestId: "r1", tool: "run", name: "api tests", command: "npm test -- --watch",
  startedAtMs: now - 2 * MIN, endedAtMs: null, state: { type: "running" } };
const step = (kind: OrchestratorStep["kind"], at: number): OrchestratorStep => ({ requestId: "r1", kind, atMs: now - at * MIN, position: 0 });
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, tasks, plans: { [plan.id]: plan },
  previews: { [page.id]: page }, backgroundRuns: { [run.id]: run },
  // The MCP servers each agent's CLI connected: the session's own model at its start, a worker's kept current.
  mcpServers: {
    lead: { taskId: null, provider: "claude", atStart: true, servers: [
      { name: "linear", status: "connected", tools: ["search_issues", "get_issue"], transport: "http" },
      { name: "figma", status: "needs-auth", tools: [], transport: null },
    ] },
    "task:task-3": { taskId: "task-3", provider: "codex", atStart: false, servers: [{ name: "postgres", status: "failed", tools: [], transport: null }] },
  },
  diffs: { "task-2": { files: [], insertions: 120, deletions: 4 }, "task-3": { files: [], insertions: 48, deletions: 0 } },
  orchestratorSteps: [
    step({ type: "searchedWeb", query: "express static files" } as OrchestratorStep["kind"], 13),
    step({ type: "tool", itemId: "t1", name: "mcp__claude_ai_Linear__search_issues", detail: "web ui", status: "completed", throughPosition: 0 } as OrchestratorStep["kind"], 12),
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

// `tab=sources` opens the Sources tab, `tab=progress` the Job card's, for a screenshot.
if (params.get("tab") === "sources") openPanelTab(id, { type: "sources" }, "Sources");
if (params.get("tab") === "progress") openPanelTab(id, { type: "reviews" }, "Progress");

const card = () => document.querySelector<HTMLElement>('[data-slot="summary-card"]');
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 50)));

/** What the card shows at rest, and what its clicks do: they open things, never grow the card. */
async function drive(): Promise<string> {
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
  seen.planInCard = card()!.querySelectorAll('[data-card="plan"]').length;
  const job = document.querySelector<HTMLElement>('[aria-label="Context"] [data-slot="job-card"]');
  seen.job = job?.querySelector('[data-slot="job-progress"] span.truncate')?.textContent ?? null;
  // The job's card comes right after the context card.
  seen.jobAfterCard = card()?.nextElementSibling?.querySelector('[data-slot="job-card"]') === job;
  const project = card()!.querySelector('[data-slot="summary-section"]')!;
  seen.project = [...project.querySelectorAll('[data-slot="summary-row"]')].map((row) => row.textContent?.replace(/\s+/g, " ").trim());
  seen.branchTip = project.querySelector('[data-row="branch"]')?.getAttribute("title");
  seen.findingsInCard = card()!.querySelectorAll('[data-slot*="finding"]').length;
  // The whole card is one open button laid over its content (the action buttons are siblings).
  job?.parentElement?.querySelector<HTMLButtonElement>(':scope > [data-slot="job-card-open"]')?.click();
  await frame();
  seen.jobOpens = sessionTabs(id).active;
  seen.branchIsButton = project.querySelector('[data-row="branch"]')?.tagName === "BUTTON";
  seen.grew = card()!.getBoundingClientRect().height !== height;
  // Sources: what the session used, the first three, then View all for its searches and pages.
  const sources = [...card()!.querySelectorAll('[data-slot="summary-section"]')].find((section) => section.querySelector("header")?.textContent?.startsWith("Sources"));
  seen.sources = [...(sources?.querySelectorAll('[data-slot="summary-row"]') ?? [])].map((row) => row.textContent?.trim());
  [...(sources?.querySelectorAll<HTMLButtonElement>('[data-slot="summary-row"]') ?? [])].find((row) => row.textContent?.trim() === "View all")?.click();
  await frame();
  seen.sourcesOpens = sessionTabs(id).active;
  seen.mcp = [...document.querySelectorAll('[data-slot="sources-mcp"]')].map((group) =>
    [group.querySelector("p")?.textContent, ...[...group.querySelectorAll('[data-slot="mcp-server-panel"] > li')].map((row) => row.querySelector("button")?.textContent)]);
  seen.sourcesTab = [...document.querySelectorAll('[data-slot="sources-part"]')].map((part) =>
    [...part.querySelectorAll("h3, [data-slot=sources-line]")].map((line) => line.textContent?.trim()));
  return JSON.stringify(seen);
}

/** The Sources tab, while it is in front, as the session's tab bar would show it. */
function FrontTab() {
  const tabs = useSessionTabsOf(id);
  const front = tabs.tabs.find((candidate) => candidate.id === tabs.active);
  if (front?.kind !== "panel") return null;
  return front.view.type === "sources" ? <SourcesTab conversationId={id} /> : front.view.type === "reviews" ? <SessionJobTab conversationId={id} /> : null;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [panel, setPanel] = useState<AgentsPanelState>(undefined);
  const [result, setResult] = useState("");
  useEffect(() => {
    if (params.has("drive")) void drive().then(setResult);
  }, []);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <AgentsPanelContext.Provider value={{ panel, setPanel }}>
      <main className="bg-background text-foreground relative flex h-screen w-full min-w-0">
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="h-titlebar flex items-center px-5 text-sm font-medium">Add the web interface</header>
          <PinnedSummary conversation={conversation} />
          <FrontTab />
        </div>
      </main>
      </AgentsPanelContext.Provider>
      {result && <pre id="context-rows-result">{result}</pre>}
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
