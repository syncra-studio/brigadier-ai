/**
 * A mockup, not the product: the session's context card in the app's window layout, with its
 * Activities section listing everything running for the session (the thread's previews, a
 * worker's long command, a dev app a worker started, the thread's own long commands). The card
 * and its other sections are the real ones; only the Activities section below is drawn here, put
 * where the real one sits. Synthetic data; requests never reach a daemon.
 *
 * Each activity is a task card (`./task-card`, a mockup-only port of assistant-ui's Task Card).
 *
 * Query `view=`: `today` (the card as it is), `build` (a worker builds; the thread's "Running"
 * section kept), `build-merged` (the same, merged into Activities), `starting` and `running` (the
 * build done, the dev app starting, then running), `open` (`build-merged` with the build's card
 * opened), `folded` (the section folded, with its count).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { type ReactNode, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip, TitlebarNav } from "@/app/AppSidebar";
import { ConversationView } from "@/app/ConversationView";
import { elapsedWords } from "@/app/conversation/liveStatus";
import { SidebarFoot } from "@/app/SidebarFoot";
import { SummarySection } from "@/components/assistant-ui/elements/summary-section";
import { Button } from "@/components/ui/button";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import { TaskCard, type TaskCardState } from "@/fixtures/task-card";
import type { BackgroundRun, Conversation, Message, Preview, Request, Task } from "@/ipc/generated";
import { type Board, emptyBoard, useBoard } from "@/state/board";
import { emptyThread, useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const view = query.get("view") ?? "build";
const today = view === "today";
// The thread's own long commands: in their own "Running" section, or merged into Activities.
const merged = view !== "today" && view !== "build";
// The worker's build is done and the dev app it built starts (then runs).
const later = view === "starting" || view === "running";

const id = "activities-mockup";
const now = Date.now();
const MIN = 60_000;
const conversation = {
  id, kind: "session", projectId: null, title: "Fix the demo pages", pinnedAtMs: null,
  createdAtMs: now - 30 * MIN, updatedAtMs: now, lifecycle: "active", forkedFrom: null, sideOf: null,
  fallback: null, quotaWait: null,
  setup: {
    type: "session", repo: "/work/brigadier",
    environment: { type: "newWorktree", branch: "brigadier/demo/session", base: "main" },
    permission: "approveForMe", planMode: false, workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
} as Conversation;

const message = (seq: number, role: "user" | "assistant", text: string): Message => ({
  id: `m${seq}`, conversationId: id, seq, role, text, blob: null, createdAtMs: now - (20 - seq) * MIN,
  attachments: [], mentions: [], model: null, requestId: "r1", parentId: null,
});
const messages = [
  message(1, "user", "Fix the demo pages, then show me the fixes in the dev app."),
  message(2, "assistant", "The demo pages are fixed and their preview is up at localhost:5199. A worker is building the dev app so you can try the fixes there; I'll tell you when it opens."),
];

const base = Object.values(night.tasks)[0] as unknown as Task;
const task = (number: number, title: string, state: Task["state"], createdAtMs: number): Task => ({
  ...base, id: `task-${number}`, number, title, conversationId: id, state, kind: "implement", role: null,
  phase: null, run: null, attempts: [], requestId: "r1", gateLink: null, candidate: null, report: null,
  quotaWait: null, blockedReason: null, review: null, createdAtMs, updatedAtMs: now,
} as Task);
const tasks = {
  "task-1": task(1, "Fix demo pages", "done", now - 25 * MIN),
  "task-2": task(2, "Demo of the fixes", "running", now - 6 * MIN),
};
const BUILD = "pnpm tauri build --debug --bundles app";
const previews: Record<string, Preview> = {
  "preview-1": {
    id: "preview-1", conversationId: id, name: "fix demo pages", command: "pnpm dev --port 5199", kind: "web",
    workdir: "/work/brigadier/apps/desktop", workspace: "/work/brigadier", pid: 42, log: null,
    url: "http://localhost:5199/", state: { type: "running" }, startedAtMs: now - 12 * MIN, endedAtMs: null,
  } as Preview,
};
const TESTS = "cargo test -p brigadier-core";
const runs: Record<string, BackgroundRun> = {
  "run-1": { id: "run-1", conversationId: id, requestId: "r1", tool: "run_check", command: TESTS,
    startedAtMs: now - 2 * MIN, endedAtMs: null, state: { type: "running" } },
};

useBoard.setState({
  board: {
    ...emptyBoard(id),
    loaded: true,
    tasks,
    // The proposal's section stands in for the real one: the real chip shows only today.
    previews: today ? previews : {},
    backgroundRuns: merged ? {} : runs,
    activity: { "task-2": view === "running" ? "Trying the fixes in the dev app" : later ? "Starting the dev app" : `$ ${BUILD}` },
    head: messages.at(-1)!.id,
  } as Board,
});
useApp.setState({
  conversations: { [id]: conversation },
  threads: { [id]: { ...emptyThread, items: messages } },
  pinnedSummary: true,
  connection: { status: "connected", daemon: null, reason: null },
});
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getPullRequest": return { method: req.method, pullRequest: null };
    case "listFiles": return { method: req.method, files: [], truncated: false };
    default: return { method: req.method };
  }
});

/** One thing running for the session, as a task card shows it. */
type Item = {
  id: string;
  /** What it is doing, in plain words. */
  label: string;
  /** "web" or "app" for the thread's previews, the worker's name for a worker's. */
  meta?: string;
  state: TaskCardState;
  startedAtMs: number;
  endedAtMs?: number;
  /** Open (a web preview's address) and Stop, while it runs. */
  open?: string;
  stop?: boolean;
  /** Under the card: the latest output line while it starts, or how it ended. */
  result?: string;
  /** In the opened card: the raw command and its output's last lines. */
  command: string;
  tail: string;
};
const DEV = "pnpm tauri dev --config src-tauri/tauri.dev.conf.json";
const items: Item[] = [
  ...(later ? [{ id: "app", label: "Brigadier Dev", meta: "Demo of the fixes", state: "working" as const, stop: true,
    startedAtMs: now - (view === "starting" ? 0.7 : 1.5) * MIN, command: DEV,
    ...(view === "starting"
      ? { result: "Starting: compiling brigadier-core (521 of 731)", tail: "VITE ready in 286 ms\nCompiling brigadier-ipc v0.1.0\nBuilding [======> ] 521/731: brigadier-core" }
      : { tail: "VITE ready in 286 ms\nFinished `dev` profile in 41.2s\nRunning `target/debug/brigadier`" }) }] : []),
  { id: "build", label: "Building the dev app", meta: "Demo of the fixes", state: later ? "done" : "working",
    startedAtMs: now - 4 * MIN, ...(later ? { endedAtMs: now - 1.5 * MIN + 30_000, result: "Built Brigadier Dev.app" } : {}),
    command: BUILD, tail: later
      ? "Finished `debug` profile in 3m 02s\nBundling Brigadier Dev.app (target/debug/bundle/macos)\nFinished 1 bundle"
      : "Compiling brigadier-core v0.1.0\nCompiling brigadier-daemon v0.1.0\nBuilding [=====>  ] 612/731: brigadier-daemon" },
  ...(merged ? [{ id: "tests", label: "Running the core tests", state: "working" as const, stop: true, startedAtMs: now - 2 * MIN,
    command: TESTS, tail: "running 412 tests\ntest manager::preview::starts_and_stops ... ok\ntest manager::landing::rebases_onto_main ... ok" }] : []),
  { id: "preview", label: "fix demo pages", meta: "web", state: "working", stop: true, open: "http://localhost:5199/", startedAtMs: now - 12 * MIN,
    command: "pnpm dev --port 5199", tail: "VITE v8.3.0  ready in 412 ms\nLocal: http://localhost:5199/\npage reload fixtures/context-card.html" },
];
const time = (item: Item) => elapsedWords((item.endedAtMs ?? now) - item.startedAtMs);
const running = items.filter((item) => item.state !== "done");

// The section sits alone in its wrapper here, so it keeps the hairline a first section has.
const HAIRLINE = "last:pb-2 last:after:block";

function ActivityCard({ item }: { item: Item }) {
  const [open, setOpen] = useState(view === "open" && item.id === "build");
  return (
    <TaskCard label={item.label} meta={item.meta} state={item.state} elapsed={time(item)} open={open} onOpenChange={setOpen}
      result={item.result}
      actions={(item.open || item.stop) && (
        <div className="flex flex-wrap items-center gap-1">
          {item.open && <Button variant="outline" size="xs">Open</Button>}
          {item.stop && <Button variant="ghost" size="xs">Stop</Button>}
          {item.open && <span className="text-muted-foreground min-w-0 truncate text-xs">{item.open.replace(/^http:\/\/|\/$/g, "")}</span>}
        </div>
      )}>
      <code className="text-foreground/90 font-mono text-xs break-words">{item.command}</code>
      <pre className="bg-muted/50 text-muted-foreground rounded-control overflow-auto p-2 font-mono text-xs whitespace-pre-wrap break-words">{item.tail}</pre>
    </TaskCard>
  );
}

/** The proposal: a task card for each thing running for the session, newest first. */
function ActivityCards() {
  return (
    <SummarySection data-activities foldKey="activities-mockup" title="Activities" className={HAIRLINE} count={items.length} defaultFolded={view === "folded"}
      summary={view === "folded" ? `${running.length} running` : undefined}>
      <div className="flex flex-col gap-1.5 py-1">
        {items.map((item) => <ActivityCard key={item.id} item={item} />)}
      </div>
    </SummarySection>
  );
}

/** Puts the proposed section first in the real card, where the real Activities section sits. */
function InCard({ children }: { children: ReactNode }) {
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  useEffect(() => {
    const timer = setInterval(() => {
      const body = document.querySelector('[data-slot="summary-card"] .py-2\\.5');
      if (!body) return;
      clearInterval(timer);
      const host = document.createElement("div");
      host.className = "contents";
      body.prepend(host);
      setSlot(host);
    }, 50);
    return () => clearInterval(timer);
  }, []);
  return slot ? createPortal(children, slot) : null;
}

/** The app's window layout (App.tsx) around the session, as the night fixture has it. */
function Page() {
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen={query.get("sidebar") !== "0"}>
          <div aria-hidden data-slot="page-surface"
            className="bg-background rounded-page shadow-page top-titlebar start-surface-inset end-surface-inset bottom-surface-inset pointer-events-none absolute" />
          <div className="relative flex min-h-0 min-w-0 flex-1 ps-surface-inset pe-surface-inset pb-surface-inset">
            <SidebarPanel strip={<AppStrip />} foot={<SidebarFoot />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="body-divider relative flex h-full min-w-0 flex-1">
              <div className="relative flex h-full min-w-0 flex-1 flex-col">
                <ConversationView selection={{ type: "conversation", id }} />
              </div>
            </main>
          </div>
          <TitlebarNav />
        </SidebarProvider>
      </div>
      {!today && <InCard><ActivityCards /></InCard>}
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);
