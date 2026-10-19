/**
 * The production context card's Background section, fed from board data and the events that
 * change it: the thread's web preview, a worker's dev app (building, then starting, then running,
 * as its preview events say), the thread's named long command and an unnamed one. With `drive=1`
 * it opens their output, stops two, ends them, and writes what it saw. With `states=1` it shows a
 * row in each state (building, starting, running, paused, smoke testing, checking) and writes how
 * each one's icon and word look and move. With `ended=1` it shows what stays once processes end
 * (Q94): failed rows with their last line, Restart and ×, finished rows until the next message;
 * it restarts, dismisses, starts one again and sends a message, and writes what it saw.
 * Synthetic IPC; no daemon.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { BackgroundRun, Conversation, EventEnvelope, Preview, Request, Task, UserRequest } from "@/ipc/generated";
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
  if (req.method === "previewLog") {
    const tail = req.previewId === "preview-11" ? "listening…\nError: listen EADDRINUSE: address already in use :::5199\n\n" : "VITE ready in 286 ms";
    return { method: req.method, tail, url: null };
  }
  if (req.method === "readBackgroundRun") return { method: req.method, output: req.runId === "run-13" ? "running 12 tests\ntest result: FAILED. 9 passed; 3 failed\n" : "done\n" };
  if (req.method === "stopPreview") calls.push(`stopPreview ${req.previewId}`);
  if (req.method === "stopBackgroundRun") calls.push(`stopBackgroundRun ${req.runId}`);
  if (req.method === "restartPreview") {
    calls.push(`restartPreview ${req.previewId}`);
    const record = useBoard.getState().board!.previews[req.previewId]!;
    setTimeout(() => send({ type: "previewUpdated", preview: { ...record, state: { type: "starting" }, startedAtMs: Date.now(), endedAtMs: null } }));
  }
  if (req.method === "restartBackgroundRun" || req.method === "dismissBackground") {
    const row = req.method === "restartBackgroundRun" ? req.runId : req.id;
    calls.push(`${req.method} ${row}`);
    setTimeout(() => send({ type: "backgroundCleared", conversationId: id, ids: [row] }));
  }
  return { method: req.method };
});

const section = () => document.querySelector<HTMLElement>("[data-activities]");
const row = (activity: string) => section()?.querySelector<HTMLElement>(`[data-activity="${activity}"]`) ?? null;
const text = (node: Element | null | undefined) => node?.textContent?.trim() ?? null;
/** A row as it reads: what it is for, its state in a word, whether its icon moves, its time, its tooltip. */
const read = (activity: string) => {
  const button = row(activity);
  const name = button?.querySelector("span.truncate");
  const state = name?.querySelector("[data-state]");
  return button && {
    label: name?.firstChild?.textContent?.trim() ?? null,
    state: text(state),
    moving: button.querySelector(".motion-safe\\:animate-spin") !== null,
    elapsed: text(button.querySelector("span.tabular-nums")),
    tip: button.title,
  };
};
/** A row's actions, shown on hover in its time's place: Stop, and Show output on a web preview that is up. */
const action = (activity: string, label: string) =>
  row(activity)?.parentElement?.querySelector<HTMLButtonElement>(`[data-slot="summary-row-action"] button[aria-label^="${label}"]`) ?? null;
const stop = (activity: string) => action(activity, "Stop");
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 30)));

/** One row in each state: a build, an app starting, the thread's page up, a paused app, an app a
 * worker smoke-tests, and a check. */
const { taskId: _worker, ...own } = app;
const states = {
  building: app,
  starting: { ...own, id: "preview-4", name: "api server", command: "node server.js", building: false, startedAtMs: now - 5 * MIN } as Preview,
  running: page,
  paused: { ...own, id: "preview-5", name: "storybook", command: "pnpm storybook", building: false, state: { type: "paused" }, startedAtMs: now - 20 * MIN } as Preview,
  "smoke testing": { ...own, id: "preview-6", name: "todo app", command: "pnpm start", building: false, state: { type: "running" }, startedAtMs: now - 30 * MIN } as Preview,
};
const smoke: Preview = { ...states["smoke testing"], id: "preview-7", taskId: "task-2", name: "todo app smoke", command: "PORT=5301 pnpm start", startedAtMs: now - MIN };

const moves = (node: Element | null | undefined) => !!node && getComputedStyle(node).animationName !== "none";

/** How each state's row looks: its icon, whether that icon moves, whether its word shimmers. */
async function readStates(): Promise<Record<string, unknown>> {
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  send(...Object.values(states).map((preview) => ({ type: "previewUpdated" as const, preview })), { type: "previewUpdated", preview: smoke },
    { type: "backgroundRunUpdated", run: tests });
  await frame();
  return Object.fromEntries([...(section()?.querySelectorAll<HTMLElement>("[data-activity]") ?? [])].map((button) => {
    const icon = button.querySelector("[data-icon]");
    const word = button.querySelector("[data-state]");
    return [word?.getAttribute("data-state"), {
      icon: icon?.getAttribute("data-icon") ?? null,
      moving: !!icon && [icon, ...icon.children].some(moves),
      green: !!icon?.querySelector(".bg-success"),
      shimmer: word?.classList.contains("shimmer") ?? false,
      sweeping: moves(word),
    }];
  }));
}

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
  // A web preview that is up opens its page in Brigadier's browser, its output one button away;
  // anything else opens what it prints in a tab. Nothing opens in the card.
  const height = section()!.getBoundingClientRect().height;
  const front = () => {
    const tabs = sessionTabs(id);
    return tabs.tabs.find((candidate) => candidate.id === tabs.active);
  };
  row("preview-1")?.click();
  await frame();
  const browser = front();
  seen.browsed = browser?.kind === "browser" ? browser.url : browser?.kind ?? null;
  seen.outputs = ["preview-1", "preview-2", "run-1"].map((activity) => !!action(activity, "Show output"));
  action("preview-1", "Show output")?.click();
  await frame();
  const tab = front();
  seen.opened = tab?.kind === "panel" ? `${tab.id} ${tab.title}` : tab?.kind ?? null;
  row("preview-2")?.click();
  await frame();
  seen.openedApp = sessionTabs(id).active;
  row("run-1")?.click();
  await frame();
  seen.openedRun = sessionTabs(id).active;
  seen.grew = section()!.getBoundingClientRect().height !== height;
  stop("preview-1")?.click();
  stop("run-1")?.click();
  for (const end = Date.now() + 3000; Date.now() < end && calls.length < 2; ) await frame();
  seen.calls = calls.toSorted();
  // What the user stopped leaves the card.
  send({ type: "previewUpdated", preview: { ...page, state: { type: "stopped", reason: "stopped by the user" }, endedAtMs: Date.now() } },
    { type: "backgroundRunUpdated", run: { ...tests, state: { type: "stopped" }, endedAtMs: Date.now() } as BackgroundRun });
  await frame();
  seen.left = [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));
  return seen;
}

/** The thread's processes that ended (Q94): a page that crashed, an api server that failed, a docs build and a lint that finished, tests that failed, a check interrupted, and one the user stopped. */
const exited = (code: number) => ({ type: "exited" as const, code, status: `exit ${code}` });
const crashed: Preview = { ...page, id: "preview-11", state: exited(1), startedAtMs: now - 9 * MIN, endedAtMs: now - 8 * MIN };
const api: Preview = { ...own, id: "preview-12", name: "api server", command: "node server.js", building: false, state: { type: "exited", code: null, status: "killed by signal 11" }, startedAtMs: now - 7 * MIN, endedAtMs: now - 6 * MIN };
const docs: Preview = { ...own, id: "preview-13", name: "docs build", command: "pnpm docs", building: false, state: exited(0), startedAtMs: now - 5 * MIN, endedAtMs: now - 4 * MIN };
const userStopped: Preview = { ...own, id: "preview-14", name: "storybook", command: "pnpm storybook", building: false, state: { type: "stopped", reason: "stopped by the user" }, startedAtMs: now - 3 * MIN, endedAtMs: now - 2 * MIN };
const failedTests: BackgroundRun = { ...tests, id: "run-13", state: { type: "finished", status: "exit 101" }, startedAtMs: now - 10 * MIN, endedAtMs: now - 9 * MIN };
const lintDone: BackgroundRun = { ...lint, id: "run-14", state: { type: "finished", status: "exit 0" }, startedAtMs: now - 11 * MIN, endedAtMs: now - 10 * MIN };
const cut: BackgroundRun = { ...lint, id: "run-15", name: "e2e", command: "pnpm e2e", state: { type: "interrupted", reason: "Brigadier quit" }, startedAtMs: now - 12 * MIN, endedAtMs: now - 11 * MIN };

/** The rows' ids, top to bottom. */
const ids = () => [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));

async function driveEnded(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  send(...[crashed, api, docs, userStopped].map((preview) => ({ type: "previewUpdated" as const, preview })),
    ...[failedTests, lintDone, cut].map((run) => ({ type: "backgroundRunUpdated" as const, run })));
  for (const end = Date.now() + 3000; Date.now() < end && !row("preview-11")?.querySelector("[data-last-line]"); ) await frame();
  seen.order = ids();
  seen.rows = Object.fromEntries(ids().map((activity) => {
    const button = row(activity!)!;
    const word = button.querySelector("[data-state]");
    const place = button.nextElementSibling?.matches('[data-slot="summary-row-action"]') ? button.nextElementSibling : null;
    const actions = [...(place?.querySelectorAll("button") ?? [])].map((each) => each.getAttribute("aria-label"));
    return [activity, {
      state: text(word), icon: button.querySelector("[data-icon]")?.getAttribute("data-icon") ?? null,
      red: word?.classList.contains("text-destructive") ?? false, shimmer: word?.classList.contains("shimmer") ?? false,
      muted: button.classList.contains("text-muted-foreground"), lastLine: text(button.querySelector("[data-last-line]")),
      elapsed: text(button.querySelector("span.tabular-nums")), actions,
    }];
  }));
  // Restart on the crashed page: its own command again, under its id; the row is the running one.
  action("preview-11", "Restart")?.click();
  // Restart on the failed tests asks the thread to run them again; × on the interrupted e2e.
  action("run-13", "Restart")?.click();
  action("run-15", "Dismiss")?.click();
  for (const end = Date.now() + 3000; Date.now() < end && (calls.length < 3 || row("run-13") || row("run-15")); ) await frame();
  await frame();
  seen.calls = calls.toSorted();
  seen.restarted = text(row("preview-11")?.querySelector("[data-state]"));
  seen.afterActions = ids();
  // The thread starts the api server again: the new one is its row.
  send({ type: "previewUpdated", preview: { ...api, id: "preview-15", command: "PORT=4100 node server.js", state: { type: "running" }, startedAtMs: Date.now(), endedAtMs: null } });
  await frame();
  seen.startedAgain = ids();
  // The next message: the finished rows leave.
  send({ type: "requestUpdated", request: { id: "r9", conversationId: id, preview: "Now the docs", state: { type: "working" }, startedAtMs: Date.now(), endedAtMs: null,
    steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false } as UserRequest });
  await frame();
  seen.afterMessage = ids();
  return seen;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [result, setResult] = useState("");
  useEffect(() => {
    if (params.has("drive")) void drive().then((seen) => setResult(JSON.stringify(seen)));
    else if (params.has("ended")) void driveEnded().then((seen) => params.has("shot") || setResult(JSON.stringify(seen)));
    // `shot=1` leaves the result off the page, for a screenshot.
    else if (params.has("states")) void readStates().then((seen) => params.has("shot") || setResult(JSON.stringify(seen)));
    else send({ type: "previewUpdated", preview: page }, { type: "backgroundRunUpdated", run: tests }, { type: "previewUpdated", preview: app },
      // `smoke=1`: the worker smoke-tests its own copy of the demo pages: a state of their row.
      ...(params.has("smoke") ? [{ type: "previewUpdated" as const, preview: { ...page, id: "preview-3", taskId: "task-2", name: "demo pages smoke", command: "PORT=5300 pnpm dev --port 5199", startedAtMs: now - MIN } }] : []));
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
