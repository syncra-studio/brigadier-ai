/**
 * The production context card's Activities section, fed from board data and the events that
 * change it: the thread's web preview, a worker's dev app (building, then starting, then running,
 * as its preview events say), the thread's named long command and an unnamed one. With `drive=1`
 * it opens, stops, opens a page and folds, and writes what it saw. Synthetic IPC; no daemon.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { BackgroundRun, Conversation, EventEnvelope, Preview, Request, Task } from "@/ipc/generated";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "activities-fixture";
const params = new URLSearchParams(location.search);
const now = Date.now();
const MIN = 60_000;
const conversation: Conversation = { id, kind: "session", title: "Fix the demo pages", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: { type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "fixture" }, permission: "askForApproval", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } }, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
const base = Object.values(night.tasks)[0] as unknown as Task;
const worker = { ...base, id: "task-2", number: 2, title: "Demo of the fixes", conversationId: id, state: "running", kind: "implement", role: null,
  phase: null, run: null, attempts: [], requestId: "r1", gateLink: null, candidate: null, report: null,
  quotaWait: null, blockedReason: null, review: null, createdAtMs: now - 6 * MIN, updatedAtMs: now } as Task;
const page: Preview = { id: "preview-1", conversationId: id, name: "demo pages", command: "pnpm dev --port 5199", kind: "web",
  workdir: "/work/session/apps/desktop", workspace: "/work/session", pid: 42, log: null, url: "http://localhost:5199/",
  state: { type: "running" }, building: false, startedAtMs: now - 12 * MIN, endedAtMs: null };
const app: Preview = { id: "preview-2", conversationId: id, taskId: "task-2", name: "dev app", kind: "app",
  command: "pnpm tauri build --debug --bundles app --config /scratch/tauri.conf.json && \"/work/task-2/target/debug/bundle/macos/Brigadier Dev.app/Contents/MacOS/brigadier-desktop\"",
  workdir: "/work/task-2", workspace: "/work/task-2", pid: 43, log: null,
  state: { type: "starting" }, building: true, startedAtMs: now - 4 * MIN, endedAtMs: null };
const tests: BackgroundRun = { id: "run-1", conversationId: id, requestId: "r1", tool: "run_check", name: "core tests",
  command: "cargo test -p brigadier-core", startedAtMs: now - 2 * MIN, endedAtMs: null, state: { type: "running" } };
const lint: BackgroundRun = { id: "run-2", conversationId: id, requestId: "r1", tool: "run", command: "pnpm lint --max-warnings 0 --cache --cache-location /scratch/lint",
  startedAtMs: now - 3 * MIN, endedAtMs: null, state: { type: "running" } };
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, tasks: { [worker.id]: worker },
  // The worker goes on with other work while its dev app builds.
  activity: { "task-2": "$ cargo clippy --workspace" } } });
useApp.setState({
  pinnedSummary: true, selection: { type: "conversation", id }, conversations: { [id]: conversation },
  info: { platform: "macos", version: "fixture", processStartMs: now, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
let seq = 0;
const send = (...events: EventEnvelope["event"][]) => applyBoardEvents(events.map((event) => {
  seq += 1;
  return { seq, stream: `conversation:${id}`, streamSeq: seq, atMs: Date.now(), event } as EventEnvelope;
}));
const calls: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "previewLog") return { method: req.method, tail: "VITE ready in 286 ms", url: null };
  if (req.method === "stopPreview") calls.push(`stopPreview ${req.previewId}`);
  if (req.method === "stopBackgroundRun") calls.push(`stopBackgroundRun ${req.runId}`);
  return { method: req.method };
});

const section = () => document.querySelector<HTMLElement>("[data-activities]");
const row = (activity: string) => section()?.querySelector<HTMLElement>(`[data-activity="${activity}"]`) ?? null;
const text = (node: Element | null | undefined) => node?.textContent?.trim() ?? null;
/** A row as it reads: what it is for, its state word, its time. */
const read = (activity: string) => {
  const card = row(activity);
  const words = card?.querySelectorAll(":scope > button span.truncate");
  return card && { label: text(words?.[0]), meta: text(words?.[1]), elapsed: text(card.querySelector(":scope > button span.tabular-nums")), state: card.getAttribute("data-state") };
};
const buttons = (activity: string) => [...(row(activity)?.querySelectorAll('[data-slot="task-card-actions"] button') ?? [])].map((button) => text(button));
const press = (activity: string, label: string) => [...(row(activity)?.querySelectorAll<HTMLButtonElement>('[data-slot="task-card-actions"] button') ?? [])]
  .find((button) => text(button) === label)?.click();
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 30)));

async function drive(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  seen.empty = !section();
  send({ type: "previewUpdated", preview: page }, { type: "backgroundRunUpdated", run: tests }, { type: "backgroundRunUpdated", run: lint },
    { type: "previewUpdated", preview: app });
  await frame();
  seen.order = [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));
  seen.rows = Object.fromEntries(["preview-1", "preview-2", "run-1", "run-2"].map((activity) => [activity, read(activity)]));
  seen.workerLine = text(document.querySelector('[data-slot="workers-summary-activity"]'));
  // The build is done and the app starts, then runs: the same row, the same start.
  const building = row("preview-2");
  send({ type: "previewUpdated", preview: { ...app, building: false } });
  await frame();
  seen.starting = read("preview-2");
  send({ type: "previewUpdated", preview: { ...app, building: false, state: { type: "running" } } });
  await frame();
  seen.running = read("preview-2");
  seen.sameRow = building !== null && row("preview-2") === building;
  // Opened: one command line, its worker named, Stop; a page that is up has Open too.
  row("preview-2")?.querySelector<HTMLButtonElement>(":scope > button")?.click();
  row("preview-1")?.querySelector<HTMLButtonElement>(":scope > button")?.click();
  row("run-1")?.querySelector<HTMLButtonElement>(":scope > button")?.click();
  await frame();
  seen.appLine = text(row("preview-2")?.querySelector('[data-slot="task-card-transcript"]'));
  seen.appButtons = buttons("preview-2");
  seen.pageButtons = buttons("preview-1");
  seen.runButtons = buttons("run-1");
  press("preview-1", "Open");
  await frame();
  const tabs = sessionTabs(id);
  const tab = tabs.tabs.find((candidate) => candidate.id === tabs.active);
  seen.opened = tab?.kind === "browser" ? tab.url : null;
  // The command line opens the session's previews, with their logs.
  row("preview-2")?.querySelector<HTMLButtonElement>('[data-slot="task-card-transcript"] button')?.click();
  await frame();
  seen.previews = !!document.querySelector('[role="dialog"][aria-label="Previews"]');
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await frame();
  press("preview-1", "Stop");
  press("run-1", "Stop");
  for (const end = Date.now() + 3000; Date.now() < end && calls.length < 2; ) await frame();
  seen.calls = calls.toSorted();
  // Folded, the header counts what runs.
  section()?.querySelector<HTMLButtonElement>("header button")?.click();
  await frame();
  seen.folded = text(section()?.querySelector("header"));
  return seen;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [result, setResult] = useState("");
  useEffect(() => {
    if (params.has("drive")) void drive().then((seen) => setResult(JSON.stringify(seen)));
    else send({ type: "previewUpdated", preview: page }, { type: "backgroundRunUpdated", run: tests }, { type: "previewUpdated", preview: app });
  }, []);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <main className="bg-background text-foreground relative flex h-screen w-full min-w-0">
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="mx-auto max-w-xl px-6 py-12 text-sm">The demo pages are fixed; a worker builds the dev app.</div>
          <PinnedSummary conversation={conversation} />
        </div>
      </main>
      {result && <pre id="activities-result">{result}</pre>}
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
