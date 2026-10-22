/** A standalone Vite entry, excluded from the app entry and production build. Synthetic data. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { type ReactNode, useEffect, useMemo } from "react";
import { createRoot } from "react-dom/client";
import { createStore } from "zustand/vanilla";

import { StackCard } from "@/app/conversation/cards/JobCard";
import { JobStack } from "@/app/conversation/JobStack";
import { SessionJobTab } from "@/app/side-panel/JobTab";
import { SummaryRow, SummarySection } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick } from "@/fixtures/input";
import type { Message, OvernightRun, PhaseStage, Plan, Request, StepSettlement, Task } from "@/ipc/generated";
import { type Board, BoardStoreContext, emptyBoard, useBoard } from "@/state/board";
import { useKeepAwake } from "@/state/keepAwake";
import { useSessionTabs } from "@/state/sessionTabs";
import { emptyThread, useApp } from "@/state/store";

// Each column is the summary's column of cards (PinnedSummary's STACK) for a session of its own,
// with an overnight run in one of its states, at a fixed clock.
const NOW = new Date(2026, 9, 21, 2, 0).getTime();
const MIN = 60_000;
const HOUR = 60 * MIN;
const WRAP = new Date(2026, 9, 21, 7, 30).getTime();
const RESUME = new Date(2026, 9, 21, 3, 0).getTime();

const PHASES = [
  { name: "Split the importer", scope: "Split the importer into a reader and a writer." },
  { name: "Move retry loop", scope: "Move the retry loop into the reader, behind one setting." },
  { name: "Drop the old path", scope: "Remove the old import path and its flag." },
].map((phase, index) => ({
  id: `phase-${index + 1}`,
  number: index + 1,
  name: phase.name,
  scope: phase.scope,
  doneWhen: [
    { id: `p${index + 1}-c1`, text: "The importer's tests pass." },
    { id: `p${index + 1}-c2`, text: "The independent review is answered." },
  ],
  dependsOn: index ? [index] : [],
}));

function overnightRun(id: string, change: Partial<OvernightRun> = {}): OvernightRun {
  return {
    id,
    conversationId: `run-${id}`,
    segment: 1,
    predecessor: null,
    planId: null,
    name: "Clean up the importer",
    words: "/overnight Clean up the importer until 07:30",
    goal: "Clean up the importer.",
    rules: "Keep the importer's public API.",
    sources: [],
    phases: PHASES,
    directives: {
      deadline: {
        type: "at",
        time: { atMs: WRAP, localDate: "2026-10-21", localTime: "07:30", day: "Wed 21 Oct", offset: "+03:00", timeZone: "Europe/Chisinau" },
      },
      only: null,
      stopAfter: null,
      skip: [],
      maxWorkers: null,
      ignored: [],
      spans: [],
    },
    problems: [],
    revision: 1,
    generation: 1,
    state: "proposed",
    windDownAtMs: null,
    workspace: null,
    verifiedCommit: null,
    gaps: [],
    obstacles: [],
    reportMessageId: null,
    reportOutcome: null,
    endCommit: null,
    merged: null,
    notification: null,
    stop: null,
    commands: [],
    createdAtMs: NOW - 8 * HOUR,
    startedAtMs: null,
    finishedAtMs: null,
    ...change,
  };
}

/** A phase as the run's plan records it: its stage, and how the thread settled it. */
type Mark = PhaseStage | StepSettlement["outcome"];

/** The run's plan, its phases marked `marks`; a settled phase says `left` of what is left. */
function runPlan(run: OvernightRun, marks: readonly Mark[], left = ""): Plan {
  return {
    id: `plan-${run.id}`,
    conversationId: run.conversationId,
    requestId: null,
    position: 0,
    title: run.name,
    body: "",
    state: { type: "approved", by: "user" },
    createdAtMs: run.createdAtMs,
    decidedAtMs: run.createdAtMs,
    steps: PHASES.map((phase, index) => {
      const mark = marks[index] ?? "pending";
      const settled = mark === "done" || mark === "partial" || mark === "blocked";
      return {
        title: phase.name,
        detail: phase.scope,
        taskId: null,
        number: phase.number,
        outline: null,
        stage: settled ? (mark === "done" ? "done" : "failed") : mark,
        startedAtMs: mark === "pending" || mark === "skipped" ? null : NOW - (3 - index) * HOUR,
        endedAtMs: null,
        ...(settled
          ? {
              settled: {
                outcome: mark,
                summary: mark === "done" ? `${phase.name}: every check passed.` : "",
                left: mark === "done" ? [] : [left],
                tip: mark === "done" ? "abc1234" : null,
                atMs: NOW - (2 - index) * HOUR,
              },
            }
          : {}),
      };
    }),
  } as Plan;
}

/** `count` workers of phase `phase` of `run`, the first `done` of them past their work. */
function workers(run: OvernightRun, phase: number, count: number, done: number, quota?: number): Task[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `${run.id}-w${index + 1}`,
    conversationId: run.conversationId,
    number: index + 1,
    title: `Worker ${index + 1}`,
    kind: "implement",
    state: index < done ? "landed" : quota ? "paused" : "running",
    requestId: `run-${run.id.slice(-8)}-g1`,
    phase,
    run: { runId: run.id },
    route: { choice: { provider: "claude" } },
    quotaWait: quota ? { resetsAtMs: quota, reason: "Claude's usage limit resets" } : null,
    attempts: [],
    gateLink: null,
    liveLook: false,
    createdAtMs: NOW - HOUR,
    updatedAtMs: NOW - HOUR,
  })) as unknown as Task[];
}

function boardOf(run: OvernightRun, plan: Plan | null, tasks: Task[] = [], change: Partial<Board> = {}): Board {
  return {
    ...emptyBoard(run.conversationId),
    loaded: true,
    overnight: { [run.id]: plan ? { ...run, planId: plan.id } : run },
    plans: plan ? { [plan.id]: plan } : {},
    tasks: Object.fromEntries(tasks.map((task) => [task.id, task])),
    ...change,
  };
}

const started = { startedAtMs: NOW - 3 * HOUR, windDownAtMs: WRAP };
const over = (hours: number, stop: OvernightRun["stop"]) => ({
  state: "finished" as const,
  startedAtMs: NOW - 8 * HOUR,
  finishedAtMs: NOW - 8 * HOUR + hours,
  stop,
});

const proposed = overnightRun("proposed");
const running = overnightRun("running", { state: "running", ...started });
const quota = overnightRun("quota", { state: "waitingQuota", ...started });
const finished = overnightRun("finished", {
  ...over(7 * HOUR + 12 * MIN, { type: "done" }),
  verifiedCommit: "def5678abc",
  reportMessageId: "report-finished",
  reportOutcome: ["All three phases are done and checked.", "The importer is ready to merge.", "1 thing waits on you"],
});
const failed = overnightRun("failed", { ...over(4 * HOUR + 5 * MIN, { type: "blocked", phaseId: "phase-2" }), verifiedCommit: "abc1234" });
const stopped = overnightRun("stopped", { ...over(3 * HOUR + 12 * MIN, { type: "stopped" }), verifiedCommit: "abc1234" });
const partial = overnightRun("partial", { ...over(5 * HOUR + 30 * MIN, { type: "deadline" }), verifiedCommit: "abc1234" });
const restricted = overnightRun("restricted", {
  state: "running",
  ...started,
  directives: { ...proposed.directives, skip: [2] },
});
const bare = overnightRun("bare", { phases: [], directives: { ...proposed.directives, deadline: { type: "untilDone" } } });
const planning = overnightRun("planning", { phases: [], state: "running", startedAtMs: NOW - 10 * MIN, windDownAtMs: null });
// A run merged earlier: the session's own work after it has the card back.
const merged = overnightRun("merged", {
  ...over(6 * HOUR, { type: "done" }),
  verifiedCommit: "def5678abc",
  merged: { verifiedCommit: "def5678abc", commit: "9876543", into: "main", atMs: NOW - 30 * MIN },
});
const building = {
  ...workers(merged, 0, 1, 0)[0]!,
  id: "change-1",
  title: "Fix the CLI",
  requestId: "r1",
  phase: null,
  run: null,
  createdAtMs: NOW - 10 * MIN,
} as unknown as Task;

const REPORT = `**All three phases are done and checked.**

The importer is ready to merge.

1 thing waits on you

### What was done
- Phase 1 · Split the importer: done at abc1234.
- Phase 2 · Move retry loop: done.
- Phase 3 · Drop the old path: done.`;

const COLUMNS: { id: string; board: Board; tab?: boolean }[] = [
  { id: "proposed", board: boardOf(proposed, null), tab: true },
  { id: "running", board: boardOf(running, runPlan(running, ["done", "building"]), workers(running, 2, 5, 2)) },
  { id: "quota", board: boardOf(quota, runPlan(quota, ["done", "building"]), workers(quota, 2, 2, 0, RESUME)) },
  { id: "finished", board: boardOf(finished, runPlan(finished, ["done", "done", "done"]), [], { waiting: { w1: {} as never } }), tab: true },
  { id: "failed", board: boardOf(failed, runPlan(failed, ["done", "blocked"], "Tests kept failing. The report says why.")) },
  { id: "stopped", board: boardOf(stopped, runPlan(stopped, ["done", "building"]), workers(stopped, 2, 7, 2)) },
  { id: "partial", board: boardOf(partial, runPlan(partial, ["done", "partial"], "The retry test is left.")) },
  { id: "restricted", board: boardOf(restricted, runPlan(restricted, ["done", "skipped", "building"])) },
  { id: "bare", board: boardOf(bare, null) },
  { id: "planning", board: boardOf(planning, null) },
  { id: "change", board: boardOf(merged, runPlan(merged, ["done", "done", "done"]), [building]) },
];

// The live column: the open conversation's board, which the run's commands answer to.
const live = overnightRun("live");
const LIVE = live.conversationId;

const calls: Request[] = [];
let failStop = true;
mockIPC((command, payload) => {
  if (command === "notification_permission") return "granted";
  if (command !== "ipc_request") return null;
  const request = (payload as { request: Request }).request;
  calls.push(request);
  const run = useBoard.getState().board?.overnight[live.id] ?? live;
  const next = (change: Partial<OvernightRun>) => ({ method: request.method, run: { ...run, ...change, revision: run.revision + 1 } });
  switch (request.method) {
    case "startOvernight":
      return next({ state: "running", ...started });
    case "stopOvernight":
      if (failStop) {
        failStop = false;
        throw new Error("The daemon is unreachable. Try again.");
      }
      return next({ state: "windingDown" });
    case "mergeOvernight":
      return next({ merged: { verifiedCommit: "def5678abc", commit: "9876543", into: "main", atMs: NOW } });
    default:
      return { method: request.method };
  }
});

useKeepAwake.setState({ status: { active: false, lidClosed: "ready", forRun: false, screenOn: false, onBattery: true, error: null } });
useApp.setState({
  threads: {
    [finished.conversationId]: {
      ...emptyThread,
      items: [{ id: "report-finished", role: "assistant", text: REPORT } as unknown as Message],
    },
  },
});
useBoard.setState({ board: boardOf(live, null) });

/** A stand-in for the context card: its surface and two of its rows. */
function ContextCard() {
  return (
    <StackCard className="pt-2 pb-2">
      <SummarySection foldKey="fixture-project" title="brigadier-ai">
        <SummaryRow meta={<span><span className="text-success">+380</span> <span className="text-destructive">−2</span></span>}>brigadier/importer</SummaryRow>
        <SummaryRow meta="2 done">Workers</SummaryRow>
      </SummarySection>
    </StackCard>
  );
}

function Column({ id, children }: { id: string; children: ReactNode }) {
  return (
    <div data-column={id} className="w-summary-stack bg-background flex shrink-0 flex-col gap-summary-inset p-summary-inset">
      {children}
    </div>
  );
}

function Session({ id, board, tab }: { id: string; board: Board; tab?: boolean | undefined }) {
  const store = useMemo(() => createStore(() => ({ board, orchestrator: null })), [board]);
  return (
    <BoardStoreContext.Provider value={store}>
      <Column id={id}>
        <ContextCard />
        <JobStack conversationId={board.conversationId} />
      </Column>
      {tab && (
        <div data-tab={id} className="bg-background w-160 p-4">
          <SessionJobTab conversationId={board.conversationId} />
        </div>
      )}
    </BoardStoreContext.Provider>
  );
}

/** What a column shows: its cards, and its job card's parts. */
function seen(column: Element) {
  const card = column.querySelector('[data-slot="job-progress"]');
  const text = (selector: string) => [...(card?.querySelectorAll(selector) ?? [])].map((node) => node.textContent);
  return {
    cards: [...column.children].map((child) =>
      child.querySelector('[data-slot="job-card"]') ? "job" : child.querySelector('[data-slot="schedule-card"]') ? "schedule" : "context",
    ),
    state: card?.getAttribute("data-state") ?? null,
    run: card?.getAttribute("data-run") ?? null,
    title: card?.querySelector("span.truncate")?.textContent ?? null,
    meta: card?.querySelector("span.tabular-nums")?.textContent ?? null,
    segments: [...(card?.querySelectorAll<HTMLElement>("[data-segment]") ?? [])].map((segment) => segment.dataset["segment"]),
    fills: [...(card?.querySelectorAll<HTMLElement>("[data-segment]") ?? [])].map((segment) => segment.querySelector("span")?.style.width ?? ""),
    line: [...(card?.querySelectorAll<HTMLElement>("[data-stage]") ?? [])].map((stage) => `${stage.dataset["stage"]}:${stage.textContent}`),
    outcome: card?.querySelector('[data-slot="job-outcome"]')?.textContent ?? null,
    detail: text('[data-slot="job-detail"]'),
    warning: card?.querySelector('[data-slot="job-warning"]')?.textContent ?? null,
    stop: card?.querySelector('button[aria-label="Stop"]') !== null,
    buttons: [...column.querySelectorAll('[data-slot="job-card"] button')].map((button) => `${button.textContent}${(button as HTMLButtonElement).disabled ? " (off)" : ""}`),
    schedule: column.querySelector('[data-slot="schedule-card"]')?.textContent ?? null,
  };
}

const frame = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));

async function until(check: () => boolean, what: string): Promise<void> {
  // Ten seconds however busy the machine: a loaded one stretches each wait.
  for (const end = performance.now() + 10_000; performance.now() < end; ) {
    if (check()) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (check()) return;
  const alert = document.querySelector('[data-column="live"] [role="alert"]')?.textContent;
  throw new Error(`Timed out waiting for ${what} (asked: ${calls.map((call) => call.method).join(", ")}${alert ? `; it said: ${alert}` : ""})`);
}

const liveColumn = () => document.querySelector('[data-column="live"]')!;
const liveButton = (words: string) =>
  [...liveColumn().querySelectorAll<HTMLButtonElement>('[data-slot="job-card"] button')].find((button) => button.textContent === words || button.getAttribute("aria-label") === words)!;
const tabOpen = () => useSessionTabs.getState().sessions[LIVE]?.active ?? null;
const methods = () => calls.map((call) => call.method).filter((method) => method.endsWith("Overnight"));

/** The live column, as the user would use it: Start, Stop (which asks), a click on the card, Merge, Edit. */
async function act(steps: Record<string, unknown>) {
  await fixtureClick(liveButton("Start"));
  await until(() => seen(liveColumn()).state === "running", "the run to start");
  steps["start"] = { methods: methods(), tab: tabOpen(), view: seen(liveColumn()) };

  await fixtureClick(liveButton("Stop"));
  await until(() => document.querySelector('[role="dialog"]') !== null, "the stop question");
  const dialog = document.querySelector('[role="dialog"]')!;
  // A click lands where the button is now: wait out the dialog's opening, which a loaded
  // machine stretches, or it misses the moving button.
  const opening = () => dialog.getAnimations({ subtree: true }).some((animation) => animation.playState === "running");
  await until(() => !opening(), "the stop question to open");
  steps["ask"] = { question: dialog.querySelector("h2")?.textContent ?? null, methods: methods(), tab: tabOpen() };
  await fixtureClick([...dialog.querySelectorAll("button")].find((button) => button.textContent === "Stop")!);
  await until(() => liveColumn().querySelector('[role="alert"]') !== null, "the stop failure");
  const alert = dialog.querySelector<HTMLElement>('[role="alert"]');
  const retry = [...dialog.querySelectorAll("button")].find((button) => button.textContent === "Stop")!;
  steps["stopError"] = {
    open: dialog.isConnected,
    error: alert?.textContent ?? null,
    destructive: alert?.classList.contains("text-destructive") ?? false,
    visible: (alert?.getClientRects().length ?? 0) > 0,
    retryEnabled: !retry.disabled,
  };
  await fixtureClick(retry);
  await until(() => methods().includes("stopOvernight") && document.querySelector('[role="dialog"]') === null, "the stop");
  await frame();
  steps["stop"] = { methods: methods(), tab: tabOpen(), view: seen(liveColumn()) };

  const board = useBoard.getState().board!;
  const run = board.overnight[live.id]!;
  useBoard.setState({ board: { ...boardOf(finished, runPlan(finished, ["done", "done", "done"])), conversationId: LIVE, overnight: { [live.id]: { ...finished, id: live.id, conversationId: LIVE, revision: run.revision + 1, planId: `plan-${finished.id}` } } } });
  await until(() => liveButton("Merge") !== undefined, "the finished run");
  await fixtureClick(liveColumn().querySelector('[data-slot="job-card-open"]')!);
  steps["open"] = { tab: tabOpen() };
  useSessionTabs.setState({ sessions: {} });
  await fixtureClick(liveButton("Merge"));
  await until(() => methods().includes("mergeOvernight"), "the merge");
  const merge = calls.find((call) => call.method === "mergeOvernight") as { verifiedCommit?: string } | undefined;
  steps["merge"] = { methods: methods(), tab: tabOpen(), verifiedCommit: merge?.verifiedCommit ?? null };

  useBoard.setState({ board: boardOf(live, null) });
  await until(() => liveButton("Edit") !== undefined, "the proposal again");
  await fixtureClick(liveButton("Edit"));
  steps["edit"] = { tab: tabOpen() };
}

function Fixture() {
  useEffect(() => {
    let done = false;
    void (async () => {
      await frame();
      const columns = Object.fromEntries([...document.querySelectorAll<HTMLElement>("[data-column]")].map((column) => [column.dataset["column"], seen(column)]));
      const tabs = Object.fromEntries(
        [...document.querySelectorAll<HTMLElement>("[data-tab]")].map((tab) => [
          tab.dataset["tab"],
          {
            parts: tab.querySelectorAll('[data-slot="run-part"]').length,
            scopes: [...tab.querySelectorAll('[data-slot="run-phase-scope"]')].map((scope) => scope.textContent),
            report: tab.querySelector('[data-slot="run-report"]')?.textContent ?? null,
            buttons: [...tab.querySelectorAll('[data-slot="run-part"] button')].map((button) => button.textContent),
          },
        ]),
      );
      const shots = new URLSearchParams(location.search).has("shots");
      const steps: Record<string, unknown> = {};
      if (!shots) await act(steps).catch((error: unknown) => (steps["error"] = String(error)));
      if (done) return;
      const result = document.createElement("pre");
      result.id = "run-card-result";
      result.textContent = JSON.stringify({ columns, tabs, steps });
      document.body.append(result);
    })();
    return () => {
      done = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="flex flex-wrap items-start gap-6 p-6">
        {/* First: the page doesn't scroll (the app's root clips), and the live card is clicked. */}
        <Column id="live">
          <ContextCard />
          <JobStack conversationId={LIVE} />
        </Column>
        {COLUMNS.map((column) => (
          <Session key={column.id} id={column.id} board={column.board} tab={column.tab} />
        ))}
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
