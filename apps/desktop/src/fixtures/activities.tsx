/**
 * The production context card's Background section, fed from board data and the events that
 * change it: the thread's web preview and a worker's dev app (building, then starting, then
 * running, as its preview events say). With `drive=1` it opens their output, stops one, ends it,
 * and writes what it saw. With `states=1` it shows a row in each state (building, starting,
 * running, paused, smoke testing, checking) and writes how each one's icon and word look and move.
 * With `ended=1` it runs processes, then ends them (failed, finished, stopped) and writes that
 * their rows left. With `tasks=1` the CLI's own background tasks come as its live stream says
 * them (a `run_in_background` command and a background subagent): their rows, Stop, the command's
 * output, and each row dropping as soon as it ends; it writes what it saw. With `names=1` it
 * measures long and short names beside state words at wide and narrow card widths.
 * Synthetic IPC; no daemon.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { OutputTab } from "@/app/side-panel/OutputTab";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import { fixtureInput } from "@/fixtures/input";
import { rowOf, showRows } from "@/fixtures/workerRows";
import type { BackgroundTask, Conversation, EventEnvelope, Preview, Request, Task } from "@/ipc/generated";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { applyThreadNews, useCliThread } from "@/state/cliThread";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "activities-fixture";
const params = new URLSearchParams(location.search);
const now = Date.now();
const MIN = 60_000;
const conversation: Conversation = { id, kind: "session", title: "Fix the demo pages", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: { type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "fixture" }, permission: "ask", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } }, lifecycle: "active",
  forkedFrom: null, sideOf: null, quotaWait: null };
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
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, tasks: { [worker.id]: worker },
  // The worker goes on with other work while its dev app builds.
  activity: { "task-2": "$ cargo clippy --workspace" } } });
// The preview's tip names its worker by the session's worker rows.
showRows(id, [rowOf(worker)]);
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
  if (command === "open_url") calls.push(`open_url ${(payload as { url: string }).url}`);
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "previewLog") return { method: req.method, tail: "VITE ready in 286 ms", url: null };
  if (req.method === "stopPreview") calls.push(`stopPreview ${req.previewId}`);
  if (req.method === "stopBackgroundTask") calls.push(`stopBackgroundTask ${req.taskId}`);
  if (req.method === "readBackgroundTaskOutput") return { method: req.method, output: "sleeping\nmarker\n" };
  return { method: req.method };
});

const section = () => document.querySelector<HTMLElement>("[data-activities]");
const row = (activity: string) => section()?.querySelector<HTMLElement>(`[data-activity="${activity}"]`) ?? null;
const text = (node: Element | null | undefined) => node?.textContent?.trim() ?? null;
/** A row as it reads: what it is for, its state in a word, whether its icon moves, its time, its tooltip. */
const read = (activity: string) => {
  const button = row(activity);
  const name = button?.querySelector("[data-activity-name]");
  const state = button?.querySelector("[data-state]");
  return button && {
    label: text(name),
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
/** The rows' ids, top to bottom. */
const ids = () => [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 30)));

/** Whether a row's actions show without the mouse over it or focus on it. */
const actionsVisible = (activity: string) => {
  const place = row(activity)?.parentElement?.querySelector<HTMLElement>('[data-slot="summary-row-action"]');
  return !!place && getComputedStyle(place).display !== "none";
};

/** One row in each state: a build, an app starting, the thread's page up, a paused app, an app a
 * worker smoke-tests, and an app a worker checks against its own copy. */
const { taskId: _worker, ...own } = app;
const states = {
  building: app,
  starting: { ...own, id: "preview-4", name: "api server", command: "node server.js", building: false, startedAtMs: now - 5 * MIN } as Preview,
  running: page,
  paused: { ...own, id: "preview-5", name: "storybook", command: "pnpm storybook", building: false, state: { type: "paused" }, startedAtMs: now - 20 * MIN } as Preview,
  "smoke testing": { ...own, id: "preview-6", name: "todo app", command: "pnpm start", building: false, state: { type: "running" }, startedAtMs: now - 30 * MIN } as Preview,
  checking: { ...own, id: "preview-8", name: "core api", command: "node api.js", building: false, state: { type: "running" }, startedAtMs: now - 2 * MIN } as Preview,
};
const smoke: Preview = { ...states["smoke testing"], id: "preview-7", taskId: "task-2", name: "todo app smoke", command: "PORT=5301 pnpm start", startedAtMs: now - MIN };
const check: Preview = { ...states.checking, id: "preview-9", taskId: "task-2", name: "core api copy", command: "PORT=5302 node api.js", startedAtMs: now - MIN };

const moves = (node: Element | null | undefined) => !!node && getComputedStyle(node).animationName !== "none";

/** How each state's row looks: its icon, whether that icon moves, whether its word shimmers. */
async function readStates(): Promise<Record<string, unknown>> {
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  send(...Object.values(states).map((preview) => ({ type: "previewUpdated" as const, preview })), { type: "previewUpdated", preview: smoke },
    { type: "previewUpdated", preview: check });
  await frame();
  return Object.fromEntries(Object.entries(states).map(([state, preview]) => {
    const button = row(preview.id);
    const icon = button?.querySelector("[data-icon]");
    const word = button?.querySelector("[data-state]");
    return [state, {
      word: text(word),
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
  send({ type: "previewUpdated", preview: page }, { type: "previewUpdated", preview: app });
  await frame();
  seen.title = text(section()?.querySelector("header button[aria-controls]"));
  seen.order = [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));
  seen.rows = Object.fromEntries(["preview-1", "preview-2"].map((activity) => [activity, read(activity)]));
  seen.heights = [...new Set([...(section()?.querySelectorAll<HTMLElement>("[data-activity]") ?? [])].map((button) => Math.round(button.getBoundingClientRect().height)))];
  seen.workerLine = text(document.querySelector('[aria-label="Session workers"]'));
  // The build is done and the app starts, then runs: the same row.
  const building = row("preview-2");
  send({ type: "previewUpdated", preview: { ...app, building: false } });
  await frame();
  send({ type: "previewUpdated", preview: { ...app, building: false, state: { type: "running" } } });
  await frame();
  seen.sameRow = building !== null && row("preview-2") === building;
  seen.stops = ["preview-1", "preview-2"].map((activity) => text(stop(activity)));
  // A running row's Stop shows only on hover or focus, not at rest.
  seen.stopVisibleAtRest = actionsVisible("preview-2");
  // A web preview that is up opens its page in Brigadier's browser, its output one button away;
  // any other preview opens what it prints in a tab. Nothing opens in the card.
  const height = section()!.getBoundingClientRect().height;
  const front = () => {
    const tabs = sessionTabs(id);
    return tabs.tabs.find((candidate) => candidate.id === tabs.active);
  };
  row("preview-1")?.click();
  await frame();
  const browser = front();
  seen.browsed = browser?.kind === "browser" ? browser.url : browser?.kind ?? null;
  seen.outputs = ["preview-1", "preview-2"].map((activity) => !!action(activity, "Show output"));
  action("preview-1", "Show output")?.click();
  await frame();
  const tab = front();
  seen.opened = tab?.kind === "panel" ? `${tab.id} ${tab.title}` : tab?.kind ?? null;
  row("preview-2")?.click();
  await frame();
  seen.openedApp = sessionTabs(id).active;
  seen.grew = section()!.getBoundingClientRect().height !== height;
  stop("preview-1")?.click();
  for (const end = Date.now() + 3000; Date.now() < end && calls.length < 1; ) await frame();
  seen.calls = calls.toSorted();
  // What the user stopped leaves the card.
  send({ type: "previewUpdated", preview: { ...page, state: { type: "stopped", reason: "stopped by the user" }, endedAtMs: Date.now() } });
  await frame();
  seen.left = [...(section()?.querySelectorAll("[data-activity]") ?? [])].map((card) => card.getAttribute("data-activity"));
  return seen;
}

/** The thread's processes running, then ended: a page that crashes, an api server that fails, a docs build that finishes, and one the user stops. */
const exited = (code: number) => ({ type: "exited" as const, code, status: `exit ${code}` });
const crashed: Preview = { ...page, id: "preview-11", name: "Brigadier dev app", startedAtMs: now - 9 * MIN };
const api: Preview = { ...own, id: "preview-12", name: "api server", command: "node server.js", building: false, state: { type: "running" }, startedAtMs: now - 7 * MIN };
const docs: Preview = { ...own, id: "preview-13", name: "docs build", command: "pnpm docs", building: false, state: { type: "running" }, startedAtMs: now - 5 * MIN };
const storybook: Preview = { ...own, id: "preview-14", name: "storybook", command: "pnpm storybook", building: false, state: { type: "running" }, startedAtMs: now - 3 * MIN };

/** The card's Restart and × buttons, wherever they are: there are none any more. */
const restartOrDismiss = () => document.querySelectorAll('[data-slot="summary-card"] button[aria-label^="Restart"], [data-slot="summary-card"] button[aria-label^="Dismiss"]').length;

async function driveEnded(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  send(...[crashed, api, docs, storybook].map((preview) => ({ type: "previewUpdated" as const, preview })));
  await frame();
  seen.running = ids();
  const ended = Date.now();
  send(...[
    { ...crashed, state: exited(1) },
    { ...api, state: { type: "exited", code: null, status: "killed by signal 11" } },
    { ...docs, state: exited(0) },
    { ...storybook, state: { type: "stopped", reason: "stopped by the user" } },
  ].map((preview) => ({ type: "previewUpdated" as const, preview: { ...preview, endedAtMs: ended } as Preview })));
  await frame();
  seen.ended = ids();
  seen.section = !!section();
  seen.restartOrDismiss = restartOrDismiss();
  return seen;
}

/** The CLI's background tasks, as Claude Code's stream says them. */
const sleep: BackgroundTask = { id: "bdza4j3ng", itemId: "toolu_bash", kind: "shell", label: "Sleep 20 seconds then echo marker", status: "running",
  startedAtMs: now - 8_000, endedAtMs: null, outputFile: "/tmp/claude/tasks/bdza4j3ng.output", summary: null, lastLine: "sleeping" };
const helper: BackgroundTask = { id: "a0c79", itemId: "toolu_agent", kind: "agent", label: "Reply with hi", status: "running",
  startedAtMs: now - 5_000, endedAtMs: null, outputFile: null, summary: null, lastLine: null };
const tasksSaid = (...background: BackgroundTask[]) => applyThreadNews({ conversationId: id, entries: [], opened: false, background });
/** Frames until `done` holds, up to `ms`; how long it took. */
async function until(done: () => boolean, ms: number): Promise<number> {
  const start = Date.now();
  while (!done() && Date.now() - start < ms) await frame();
  return Date.now() - start;
}

async function driveTasks(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  useCliThread.setState({ views: { [id]: { entries: [], turns: [] } } });
  tasksSaid(sleep, helper);
  await until(() => ids().length === 1, 3000);
  seen.running = ids();
  seen.rows = Object.fromEntries(ids().map((activity) => [activity, read(activity!)]));
  seen.stops = ids().map((activity) => text(stop(activity!)));
  seen.buttons = ids().map((activity) => row(activity!)?.tagName ?? null);
  seen.nativeWorkers = [...document.querySelectorAll('[data-slot="workers-summary-worker"]')].map((element) => element.textContent);
  // A command opens its output in a tab; agents are listed in Workers.
  row("task:bdza4j3ng")?.click();
  await frame();
  seen.opened = sessionTabs(id).active;
  seen.output = await until(() => !!document.querySelector('[data-slot="output-tab"]')?.textContent?.includes("marker"), 3000) < 3000;
  stop("task:bdza4j3ng")?.click();
  await until(() => calls.length > 0, 3000);
  seen.calls = calls.toSorted();
  // A command that just completed leaves immediately. Native agents stay in Workers.
  tasksSaid({ ...sleep, status: "completed", endedAtMs: Date.now() }, { ...helper, status: "completed", endedAtMs: Date.now(), summary: "hi" });
  await frame();
  seen.completed = ids();
  tasksSaid({ ...sleep, id: "b2", status: "stopped", endedAtMs: Date.now() }, { ...helper, id: "a2", status: "failed", endedAtMs: Date.now() });
  await frame();
  seen.stoppedOrFailed = ids();
  seen.section = !!section();
  return seen;
}

/** Long names use the available room; state words stay whole, directly beside short names. */
async function readNames(): Promise<Record<string, unknown>> {
  for (const end = Date.now() + 5000; Date.now() < end && !document.querySelector('[data-slot="summary-card"]'); ) await frame();
  const longName = "Close worker 1, wait for worker 2 and collect all their results before continuing with the remaining work. ".repeat(4);
  send(...[
    app,
    { ...app, id: "long-building", name: longName, command: "pnpm build" },
    { ...page, id: "long-running", name: longName },
  ].map((preview) => ({ type: "previewUpdated" as const, preview })));
  useCliThread.setState({ views: { [id]: { entries: [], turns: [], background: [
    { ...sleep, label: longName },
    { ...sleep, id: "plain-task", label: longName, outputFile: null },
  ] } } });
  const measure = (activity: string) => {
    const button = row(activity)!;
    const name = button.querySelector<HTMLElement>("[data-activity-name]")!;
    const word = button.querySelector<HTMLElement>("[data-state]");
    const words = name.parentElement!.getBoundingClientRect();
    const rect = word?.getBoundingClientRect();
    const meta = button.querySelector(".tabular-nums")?.getBoundingClientRect();
    return {
      truncated: name.scrollWidth > name.clientWidth,
      ellipsis: getComputedStyle(name).textOverflow,
      state: text(word),
      stateFits: !!word && word.scrollWidth <= word.clientWidth && rect!.left >= words.left && rect!.right <= words.right + 1,
      gap: rect ? rect.left - name.getBoundingClientRect().right : null,
      shimmer: word?.classList.contains("shimmer") ?? false,
      sweeping: moves(word),
      green: !!button.querySelector(".bg-success"),
      height: button.getBoundingClientRect().height,
      beforeTime: !!meta && words.right <= meta.left,
    };
  };
  const seen: Record<string, unknown> = {};
  for (const width of [800, 420]) {
    await fixtureInput({ type: "viewport", width, height: 976 });
    await frame();
    seen[width] = Object.fromEntries(["preview-2", "long-building", "long-running", "task:bdza4j3ng", "task:plain-task"].map((activity) => [activity, measure(activity)]));
  }
  // Hover replaces time with Stop; the full state word still fits in the narrower name space.
  const rect = row("long-building")!.getBoundingClientRect();
  await fixtureInput({ type: "pointer", x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 });
  await frame();
  seen.hover = { ...measure("long-building"), stopVisible: actionsVisible("long-building") };
  return seen;
}

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [result, setResult] = useState("");
  useEffect(() => {
    if (params.has("drive")) void drive().then((seen) => setResult(JSON.stringify(seen)));
    else if (params.has("tasks")) void driveTasks().then((seen) => setResult(JSON.stringify(seen)));
    else if (params.has("ended")) void driveEnded().then((seen) => setResult(JSON.stringify(seen)));
    else if (params.has("names")) void readNames().then((seen) => setResult(JSON.stringify(seen)));
    // `shot=1` leaves the result off the page, for a screenshot.
    else if (params.has("states")) void readStates().then((seen) => params.has("shot") || setResult(JSON.stringify(seen)));
    else send({ type: "previewUpdated", preview: page }, { type: "previewUpdated", preview: app },
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
        {/* The tab a command's row opens, as the side panel shows it. */}
        {params.has("tasks") && <OutputTab conversationId={id} view={{ type: "output", source: "task", id: sleep.id }} />}
      </main>
      {result && <pre id="activities-result">{result}</pre>}
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider></TooltipProvider>);
