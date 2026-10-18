/**
 * A mockup, not the product: the session's context card in the app's window layout, with its
 * Activities section listing everything running for the session (the thread's previews, a
 * worker's long command, a dev app a worker started, the thread's own long commands). The card
 * and its other sections are the real ones; only the Activities section below is drawn here, put
 * where the real one sits. Synthetic data; requests never reach a daemon.
 *
 * Each activity is a task card (`@/components/assistant-ui/elements/task-card`).
 *
 * Query `view=`: `today` (the card as it is), `build` (a worker builds the dev app; the thread's
 * "Running" section kept), `build-merged` (the same, merged into Activities), `starting` and
 * `running` (the dev app starting, then running), `open` (`build-merged` with the dev app's and the
 * demo pages' rows opened), `folded` (the section folded, with its count).
 *
 * With `real=1`, the product's own Activities section draws each view from the board data it
 * stands for (the worker's dev app a preview of its own, the tests a named background run), for
 * comparing the two.
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
import { TaskCard, type TaskCardState } from "@/components/assistant-ui/elements/task-card";
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
const real = query.has("real");

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
// What the views stand for, as the board has it: the dev app is one preview the worker started,
// building, then starting, then running; the tests a background run the thread named.
const APP = "/work/brigadier/target/debug/bundle/macos/Brigadier Dev.app/Contents/MacOS/brigadier-desktop";
const boardPreviews: Record<string, Preview> = {
  "preview-1": { ...previews["preview-1"]!, name: "demo pages" },
  "preview-2": {
    id: "preview-2", conversationId: id, taskId: "task-2", name: "dev app", kind: "app",
    command: `${BUILD} --config /scratch/tauri.conf.json && "${APP}" --brigadier-data-dir /scratch/data`,
    workdir: "/work/task-2", workspace: "/work/task-2", pid: 43, log: null,
    state: view === "running" ? { type: "running" } : { type: "starting" }, building: !later,
    startedAtMs: later ? now - 5.5 * MIN : now - 4 * MIN, endedAtMs: null,
  },
};
const boardRuns: Record<string, BackgroundRun> = { "run-1": { ...runs["run-1"]!, name: "Core tests" } };

useBoard.setState({
  board: {
    ...emptyBoard(id),
    loaded: true,
    tasks,
    // The proposal's section stands in for the real one: the real chip shows only today.
    previews: real ? boardPreviews : today ? previews : {},
    backgroundRuns: real ? boardRuns : merged ? {} : runs,
    activity: { "task-2": view === "running" ? "Trying the fixes in the dev app" : later ? "Starting the dev app" : `$ ${BUILD}` },
    head: messages.at(-1)!.id,
  } as Board,
});
useApp.setState({
  conversations: { [id]: conversation },
  threads: { [id]: { ...emptyThread, items: messages } },
  selection: { type: "conversation", id },
  summaryShown: { [id]: true },
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
  /** What it is for: "Dev app", "Demo pages". */
  label: string;
  /** Its state in a word: Building, Starting, Running, Showing. */
  status: string;
  state: TaskCardState;
  startedAtMs: number;
  /** Open (a page's address) and Stop. */
  open?: boolean;
  /** Opened: the command running now, on one line. */
  command: string;
};
const DEV = "pnpm tauri dev --config src-tauri/tauri.dev.conf.json";
const items: Item[] = [
  // One row for the dev app, from its build to the app running: the build is how it starts.
  later
    ? { id: "app", label: "Dev app", status: view === "starting" ? "Starting" : "Running", state: "working",
        startedAtMs: now - 5.5 * MIN, command: DEV, open: view === "running" }
    : { id: "app", label: "Dev app", status: "Building", state: "working", startedAtMs: now - 4 * MIN, command: BUILD },
  ...(merged ? [{ id: "tests", label: "Core tests", status: "Running", state: "working" as const, startedAtMs: now - 2 * MIN, command: TESTS }] : []),
  { id: "preview", label: "Demo pages", status: "Showing", state: "working", open: true, startedAtMs: now - 12 * MIN, command: "pnpm dev --port 5199" },
];
const time = (item: Item) => elapsedWords(now - item.startedAtMs);
const running = items.filter((item) => item.state !== "done");

// The section sits alone in its wrapper here, so it keeps the hairline a first section has.
const HAIRLINE = "last:pb-2 last:after:block";

function ActivityCard({ item }: { item: Item }) {
  const [open, setOpen] = useState(view === "open" && item.id !== "tests");
  return (
    <TaskCard label={item.label} meta={item.status} state={item.state} elapsed={time(item)} open={open} onOpenChange={setOpen}
      actions={<>
        {item.open && <Button variant="ghost" size="xs">Open</Button>}
        <Button variant="ghost" size="xs">Stop</Button>
      </>}>
      <code className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-xs">{item.command}</code>
    </TaskCard>
  );
}

/** The proposal: a task card for each thing running for the session, newest first. */
function ActivityCards() {
  return (
    <SummarySection data-activities foldKey="activities-mockup" title="Activities" className={HAIRLINE} count={items.length} defaultFolded={view === "folded"}
      summary={view === "folded" ? `${running.length} running` : undefined}>
      <div className="flex flex-col gap-0.5">
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

/** Opens or folds the real section's rows as the view has them. */
function Drive() {
  useEffect(() => {
    const timer = setInterval(() => {
      const section = document.querySelector('[data-activities]');
      if (!section) return;
      clearInterval(timer);
      if (view === "folded") section.querySelector<HTMLButtonElement>("header button")?.click();
      if (view === "open") for (const row of ["preview-2", "preview-1"]) section.querySelector<HTMLButtonElement>(`[data-activity="${row}"] > button`)?.click();
    }, 50);
    return () => clearInterval(timer);
  }, []);
  return null;
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
      {real ? <Drive /> : !today && <InCard><ActivityCards /></InCard>}
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);
