/**
 * The production context card's Background section, fed from board data and the events that
 * change it: the thread's web preview, a worker's dev app (building, then starting, then running,
 * as its preview events say), the thread's named long command and an unnamed one. With `drive=1`
 * it opens their output, stops two, ends them, and writes what it saw. Synthetic IPC; no daemon.
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
  summaryShown: { [id]: true }, selection: { type: "conversation", id }, conversations: { [id]: conversation },
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
/** A row as it reads: what it is for, its time, its tooltip. */
const read = (activity: string) => {
  const button = row(activity);
  return button && { label: text(button.querySelector("span.truncate")), elapsed: text(button.querySelector("span.tabular-nums")), tip: button.title };
};
/** A row's one action, shown on hover in its time's place. */
const stop = (activity: string) => row(activity)?.parentElement?.querySelector<HTMLButtonElement>('[data-slot="summary-row-action"] button') ?? null;
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 30)));

async function drive(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  seen.empty = !section();
  send({ type: "previewUpdated", preview: page }, { type: "backgroundRunUpdated", run: tests }, { type: "backgroundRunUpdated", run: lint },
    { type: "previewUpdated", preview: app });
  await frame();
  seen.title = text(section()?.querySelector("header button[aria-controls]"));
  seen.order = [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));
  seen.rows = Object.fromEntries(["preview-1", "preview-2", "run-1", "run-2"].map((activity) => [activity, read(activity)]));
  seen.heights = [...new Set([...(section()?.querySelectorAll<HTMLElement>("[data-activity]") ?? [])].map((button) => Math.round(button.getBoundingClientRect().height)))];
  seen.workerLine = text(document.querySelector('[aria-label="Session workers"]'));
  // The build is done and the app starts, then runs: the same row.
  const building = row("preview-2");
  send({ type: "previewUpdated", preview: { ...app, building: false } });
  await frame();
  send({ type: "previewUpdated", preview: { ...app, building: false, state: { type: "running" } } });
  await frame();
  seen.sameRow = building !== null && row("preview-2") === building;
  seen.stops = ["preview-1", "preview-2", "run-1", "run-2"].map((activity) => text(stop(activity)));
  // A click opens what it prints in a tab; nothing opens in the card.
  const height = section()!.getBoundingClientRect().height;
  row("preview-1")?.click();
  await frame();
  const tabs = sessionTabs(id);
  const tab = tabs.tabs.find((candidate) => candidate.id === tabs.active);
  seen.opened = tab?.kind === "panel" ? `${tab.id} ${tab.title}` : tab?.kind ?? null;
  row("run-1")?.click();
  await frame();
  seen.openedRun = sessionTabs(id).active;
  seen.grew = section()!.getBoundingClientRect().height !== height;
  stop("preview-1")?.click();
  stop("run-1")?.click();
  for (const end = Date.now() + 3000; Date.now() < end && calls.length < 2; ) await frame();
  seen.calls = calls.toSorted();
  // What ends leaves the card.
  send({ type: "previewUpdated", preview: { ...page, state: { type: "stopped", reason: "the user stopped it" }, endedAtMs: Date.now() } },
    { type: "backgroundRunUpdated", run: { ...tests, state: { type: "finished", status: "exit 0" }, endedAtMs: Date.now() } as BackgroundRun });
  await frame();
  seen.left = [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));
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
