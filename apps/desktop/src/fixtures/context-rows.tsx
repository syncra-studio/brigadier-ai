/**
 * The production context card of a worktree session with something in every section, in the
 * reference order: the project and its branch row, the workers (two at work, one done), what
 * runs in the background and the sources. For screenshots and render checks; synthetic IPC, no daemon. `?workers=5` puts five at work (one row).
 * The rows read the session's own CLI items and its workers' rows (S14). `?waiting=1` has its
 * workers wait on the user (a permission, a question, a sign-in); `?limit=usage` or
 * `?limit=credits` gives its last turn that limit, with the session's wait for it (MA5).
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { ResumesAtCard } from "@/app/conversation/cards/ResumesAtCard";
import { SourcesTab } from "@/app/side-panel/SourcesTab";
import { AgentsPanelContext, type AgentsPanelState } from "@/app/conversation/WorkerChip";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { Conversation, FileEntry, QuotaWait, Request, Task, ThreadTurn, WorkerWait } from "@/ipc/generated";
import { rowOf, showRows } from "@/fixtures/workerRows";
import { emptyBoard, useBoard } from "@/state/board";
import { openPanelTab, sessionTabs, useSessionTabsOf } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "context-rows-fixture";
const params = new URLSearchParams(location.search);
const now = Date.now();
const MIN = 60_000;
const limit = params.get("limit");
const RESET = now + 95 * 60_000;
const quotaWait: QuotaWait | null = limit === "usage"
  ? { reason: "Claude's 5-hour window resets", resetsAtMs: RESET, rule: null, ranking: null, sinceMs: now, limitKind: "usageWindow" }
  : null;
const conversation: Conversation = { id, kind: "session", title: "Add the web interface", projectId: "p1",
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait,
  setup: { type: "session", repo: "/work/todo-app", environment: { type: "newWorktree", branch: "brigadier/web-ui", base: "main", path: "/work/todo-app-web-ui", start: null },
    permission: "ask", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } } };
const base = Object.values(night.tasks)[0] as unknown as Task;
const worker = (taskId: string, number: number, title: string, state: Task["state"], minutes: number): Task => ({ ...base, id: taskId, number, title, conversationId: id, state,
  kind: "implement", role: null, attempts: [], requestId: "r1", gateLink: null, candidate: null, report: null, landed: state === "landed",
  quotaWait: null, blockedReason: null, review: null, createdAtMs: now - minutes * MIN, updatedAtMs: now }) as unknown as Task;
const working = Number(params.get("workers") ?? 2);
const names = ["Web page", "API routes", "Styles", "Tests", "Readme"];
const tasks = Object.fromEntries([
  ...names.slice(0, working).map((name, index) => worker(`task-${index + 2}`, index + 2, name, "running", 6 - index)),
  worker("task-1", 1, "Data model", "landed", 20),
].map((task) => [task.id, task]));
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, tasks } });
// The session's own CLI: what it called and fetched, its MCP servers, the dev server it runs in
// the background, and its workers' rows.
const tool = (itemId: string, name: string, input: unknown, at: number): FileEntry =>
  ({ event: { type: "toolCall", itemId, name, input: JSON.stringify(input), status: "completed", output: null }, atMs: now - at * MIN });
const entries: FileEntry[] = [
  { event: { type: "message", itemId: "u1", role: "user", text: "Add a web interface to the todo app" }, atMs: now - 14 * MIN },
  tool("s1", "WebSearch", { query: "express static files" }, 13),
  tool("t1", "mcp__linear__search_issues", { query: "web ui" }, 12),
  tool("f1", "WebFetch", { url: "https://expressjs.com/en/starter/static-files.html", prompt: "How?" }, 12),
  tool("f2", "WebFetch", { url: "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/form", prompt: "Forms" }, 11),
];
const turns: ThreadTurn[] = [{ userMessage: "u1", startedAtMs: now - 14 * MIN, endedAtMs: null, end: null, error: null, requests: ["r1"],
  ...(limit === "usage" && { limit: { kind: "usageWindow", resetsAtMs: RESET, window: "five_hour" } }),
  ...(limit === "credits" && { limit: { kind: "credits", resetsAtMs: null, window: null } }) }];
const waits: WorkerWait[][] = params.has("waiting")
  ? [[{ kind: "permission", what: "npm install express", card: null }], [{ kind: "question", what: "Which port should the server use?", card: null }, { kind: "signIn", what: "figma", card: null }]]
  : [];
const diffs: Record<string, { insertions: number; deletions: number }> = { "task-2": { insertions: 120, deletions: 4 }, "task-3": { insertions: 48, deletions: 0 } };
showRows(id, Object.values(tasks).map((task, index) => rowOf(task, {
  waiting: waits[index] ?? [],
  ...(task.id === "task-3" && { mcp: [{ name: "postgres", status: "failed", tools: [], transport: null }] }),
}, diffs[task.id] ? { files: [], ...diffs[task.id]! } : null)), {
  entries,
  turns,
  mcp: [
    { name: "linear", status: "connected", tools: ["search_issues", "get_issue"], transport: "http" },
    { name: "figma", status: "needs-auth", tools: [], transport: null },
  ],
  background: [{ id: "b1", itemId: "c9", kind: "shell", label: "npm run dev", status: "running", startedAtMs: now - 3 * MIN, endedAtMs: null, outputFile: "/tmp/b1.out", summary: null, lastLine: "listening on :3000" }],
});
useApp.setState({
  summaryShown: { [id]: true }, selection: { type: "conversation", id }, conversations: { [id]: conversation },
  projects: { p1: { id: "p1", name: "todo-app" } } as never,
  info: { platform: "macos", version: "fixture", processStartMs: now, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
/** Which worker the Workers tab shows, as the fixture last drew. */
let panelShown: string | null = null;
/** The "Resumes at" card's requests, in order. */
const asked: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "getSessionDiff") return { method: req.method, stat: { files: [], insertions: 380, deletions: 2 }, commits: 4 };
  if (req.method === "continueWaiting" || req.method === "dontResume") {
    asked.push(req.method);
    if (req.method === "dontResume") useApp.setState((state) => ({ conversations: { ...state.conversations, [id]: { ...state.conversations[id]!, quotaWait: null } } }));
  }
  return { method: req.method };
});

// `tab=sources` opens the Sources tab, for a screenshot.
if (params.get("tab") === "sources") openPanelTab(id, { type: "sources" }, "Sources");

const card = () => document.querySelector<HTMLElement>('[data-slot="summary-card"]');
/** The "Resumes at" card's words. */
const wait = () => document.querySelector<HTMLElement>('[data-slot="schedule-card"]');
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
  const project = card()!.querySelector('[data-slot="summary-section"]')!;
  seen.project = [...project.querySelectorAll('[data-slot="summary-row"]')].map((row) => row.textContent?.replace(/\s+/g, " ").trim());
  seen.branchTip = project.querySelector('[data-row="branch"]')?.getAttribute("title");
  seen.findingsInCard = card()!.querySelectorAll('[data-slot*="finding"]').length;
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
  // Waiting on you: each worker's wait, under its name; a click opens that worker.
  const waiting = [...card()!.querySelectorAll<HTMLButtonElement>('[data-slot="waiting-row"]')];
  seen.waiting = waiting.map((row) => [row.dataset.wait, row.textContent?.replace(/\s+/g, " ").trim()]);
  waiting.at(-1)?.click();
  await frame();
  seen.waitingOpens = panelShown;
  // The "Resumes at" card, its buttons and what they ask.
  seen.wait = wait()?.textContent ?? null;
  seen.waitButtons = [...document.querySelectorAll('[data-slot="wait-card-actions"] button')].map((button) => button.textContent?.trim());
  document.querySelector<HTMLButtonElement>('[data-slot="continue-now"]')?.click();
  await frame();
  document.querySelector<HTMLButtonElement>('[data-slot="dont-resume"]')?.click();
  if (limit === "usage") for (let i = 0; i < 20 && wait(); i++) await frame();
  seen.asked = asked;
  seen.waitAfter = wait()?.textContent ?? null;
  return JSON.stringify(seen);
}

/** The Sources tab, while it is in front, as the session's tab bar would show it. */
function FrontTab() {
  const tabs = useSessionTabsOf(id);
  const front = tabs.tabs.find((candidate) => candidate.id === tabs.active);
  if (front?.kind !== "panel") return null;
  return front.view.type === "sources" ? <SourcesTab conversationId={id} /> : null;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [panel, setPanel] = useState<AgentsPanelState>(undefined);
  useEffect(() => {
    panelShown = panel ?? null;
  }, [panel]);
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
          <PinnedSummary conversation={conversation} cards={<ResumesAtCard conversationId={id} />} />
          <FrontTab />
        </div>
      </main>
      </AgentsPanelContext.Provider>
      {result && <pre id="context-rows-result">{result}</pre>}
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
