/** A standalone Vite entry, excluded from the app entry and production build. Synthetic data. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { type ReactNode, useEffect, useMemo } from "react";
import { createRoot } from "react-dom/client";
import { createStore } from "zustand/vanilla";

import { StackCard } from "@/app/conversation/cards/JobCard";
import { WaitCard } from "@/app/conversation/cards/ResumesAtCard";
import { type Job } from "@/app/conversation/job";
import { JobStack } from "@/app/conversation/JobStack";
import { JobTimeline } from "@/app/side-panel/JobTab";
import { SummaryRow, SummarySection } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import type { CiRun, OrchestratorStep, Plan, Preview, Question, Request, Task, UserRequest } from "@/ipc/generated";
import { type Board, BoardStoreContext, emptyBoard, type ShownApproval } from "@/state/board";

// Each column is the summary's column of cards (PinnedSummary's STACK) for a session of its own,
// its board holding the events of one moment. Times are a few minutes back from now.
const NOW = Date.now();
const MIN = 60_000;
const T0 = NOW - 20 * MIN;

// The cards send nothing: their buttons are the Background rows'.
mockIPC((command, payload) => (command === "ipc_request" ? { method: (payload as { request: Request }).request.method } : null));

const request = (id: string, startedAtMs: number, state: UserRequest["state"], endedAtMs: number | null = null): UserRequest =>
  ({ id, conversationId: "c", preview: "", state, startedAtMs, endedAtMs, steeredInto: null, steeredAfter: null, undo: null, worked: [] }) as unknown as UserRequest;
const step = (atMs: number, kind: unknown): OrchestratorStep => ({ requestId: "r1", kind: kind as OrchestratorStep["kind"], atMs, position: 0 });
const ciRun = (workflow: string, status: string, conclusion: string, jobsDone = 0, jobsTotal = 0): CiRun =>
  ({ workflow, url: `https://github.com/o/r/actions/runs/${workflow}`, status, conclusion, jobsDone, jobsTotal }) as unknown as CiRun;
const ci = (atMs: number, runs: CiRun[], result?: string) =>
  step(atMs, { type: "ciWatch", repo: "o/r", branch: "main", sha: "4e1c49e99aa", waiting: runs.length === 0, runs, ...(result ? { result } : {}) });
const task = (id: string, state: Task["state"], extra: Partial<Task> = {}): Task =>
  ({ id, number: 1, title: "Shared MCP and skills", kind: "implement", state, requestId: "r1", subject: null, error: null, createdAtMs: T0 + MIN, updatedAtMs: T0 + 2 * MIN, ...extra }) as unknown as Task;
const byId = <T extends { id: string }>(items: T[]) => Object.fromEntries(items.map((item) => [item.id, item]));

const working = { requests: byId([request("r1", T0, { type: "working" })]) };
const ended = { requests: byId([request("r1", T0, { type: "done" }, NOW - MIN)]) };
const merged = (head: string) =>
  step(T0 + 9 * MIN, { type: "merged", branch: "brigadier/s", base: "main", commits: 4, askedIn: null, head, mergeCommit: "4e1c49e0001" });
const question: Question = { id: "q1", kind: { type: "orchestrator" }, answer: null, answeredAtMs: null, answers: [], createdAtMs: T0 + 5 * MIN, items: [{ question: "Build the page as drawn?", options: [] }] } as unknown as Question;
const plan = {
  id: "p1", conversationId: "c", requestId: "r1", position: 0, title: "Settings", body: "# Settings", state: { type: "approved" }, createdAtMs: T0, decidedAtMs: T0,
  steps: [
    { title: "Move settings", detail: null, taskId: "t1", stage: "building", startedAtMs: T0 + MIN, endedAtMs: null, outline: null },
    { title: "Delete old store", detail: null, taskId: null, stage: "pending", startedAtMs: null, endedAtMs: null, outline: null },
    { title: "Check", detail: null, taskId: null, stage: "pending", startedAtMs: null, endedAtMs: null, outline: null },
  ],
} as unknown as Plan;

/** Each moment: the board's events, and the thread's headline where it set one. */
const MOMENTS: Record<string, Partial<Board>> = {
  // First: the page doesn't scroll (the app's root clips), and their buttons are clicked.
  ciFailed: { ...ended, orchestratorSteps: [ci(T0 + 3 * MIN, [ciRun("test", "completed", "failure", 5, 5)], "failed")] },
  idle: ended,
  working: { ...working, tasks: byId([task("t1", "running")]) },
  ciChecking: { ...working, orchestratorSteps: [ci(T0 + 3 * MIN, [ciRun("test", "in_progress", "", 3, 5)])] },
  merging: { ...working, orchestratorSteps: [step(T0 + 9 * MIN, { type: "mergeRequested", id: "m1", branch: "brigadier/s", base: "main", origin: {} })] },
  merged: { ...ended, orchestratorSteps: [merged("head111")] },
  waitingAnswer: { ...working, questions: { q1: question } },
  waitingApproval: { ...working, approvals: { a1: { id: "a1", state: { type: "pending" }, createdAtMs: T0 + 5 * MIN, subject: { type: "cli" } } as unknown as ShownApproval } },
  workerFailed: { ...working, tasks: byId([task("t1", "failed", { error: "The registry tests failed" })]) },
  devServer: {
    ...working,
    previews: byId([{ id: "p1", name: "The dev server", state: { type: "exited", code: 1, status: "Port 1420 is already in use" }, startedAtMs: T0 + MIN, endedAtMs: T0 + 2 * MIN } as unknown as Preview]),
  },
  stopped: { requests: byId([request("r1", T0, { type: "stopped" }, NOW - 2 * MIN)]) },
  headline: {
    ...working,
    tasks: byId([task("t1", "running")]),
    headline: { text: "Investigating the dev server", detail: "Looking for what holds port 1420", failed: false, atMs: T0 + 6 * MIN },
  },
  plan: { ...working, tasks: byId([task("t1", "running")]), plans: { p1: plan } },
  // A plan's card: the thread's headline over the phase bar.
  planHeadline: {
    ...working,
    tasks: byId([task("t1", "running")]),
    plans: { p1: plan },
    headline: { text: "Investigating the dev server", detail: "Looking for what holds port 1420", failed: false, atMs: T0 + 6 * MIN },
  },
  // Merged, then CI that GitHub marked cancelled (a cache save timed out), then the thread's words
  // that it passed: a success, never the failure mark.
  planCiCancelled: {
    ...ended,
    tasks: byId([task("t1", "done")]),
    plans: { p1: plan },
    orchestratorSteps: [merged("head111"), ci(T0 + 10 * MIN, [ciRun("CI", "completed", "cancelled", 6, 6)], "failed")],
    headline: { text: "Merged, pushed, CI passes", detail: "Windows only timed out saving its build cache", failed: false, atMs: T0 + 11 * MIN },
  },
  // The thread's words tell of a failure that still stands: the failure mark.
  planFailedHeadline: {
    ...ended,
    tasks: byId([task("t1", "done")]),
    plans: { p1: plan },
    headline: { text: "The tests still fail", detail: "Next: ask you how to go on", failed: true, atMs: T0 + 7 * MIN },
  },
};

/** A stand-in for the context card: its surface and two of its rows. */
function ContextCard() {
  return (
    <StackCard className="pt-2 pb-2">
      <SummarySection foldKey="fixture-project" title="brigadier-ai">
        <SummaryRow meta={<span><span className="text-success">+380</span> <span className="text-destructive">−2</span></span>}>brigadier/web-ui</SummaryRow>
        <SummaryRow meta="working">Shared MCP and skills</SummaryRow>
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

function Session({ id, parts }: { id: string; parts: (typeof MOMENTS)[string] }) {
  const store = useMemo(() => {
    const board: Board = { ...emptyBoard(`session-${id}`), loaded: true, ...parts };
    return createStore(() => ({ board, orchestrator: null }));
  }, [id, parts]);
  return (
    <BoardStoreContext.Provider value={store}>
      <Column id={id}>
        <ContextCard />
        <JobStack conversationId={`session-${id}`} />
      </Column>
    </BoardStoreContext.Provider>
  );
}

const FAILED: Job = {
  steps: [
    { id: "build", name: "Building", state: "done", startedAtMs: NOW - 50 * MIN, summary: "Two workers built it" },
    { id: "ci", name: "CI", state: "failed", failedLabel: "CI failed", startedAtMs: NOW - 9 * MIN, summary: "2 tests failed in the desktop app" },
    { id: "merge", name: "Merging", state: "pending" },
  ],
};

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function Fixture() {
  useEffect(() => {
    let done = false;
    void (async () => {
      await sleep(100);
      const columns = [...document.querySelectorAll<HTMLElement>("[data-column]")];
      const seen = Object.fromEntries(
        columns.map((column) => {
          const card = column.querySelector('[data-slot="job-card"]');
          return [
            column.dataset["column"],
            {
              cards: [...column.children].map((child) =>
                child.querySelector('[data-slot="job-card"]') ? "job" : child.querySelector('[data-slot="schedule-card"]') ? "schedule" : "context",
              ),
              tone: card?.querySelector<HTMLElement>('[data-slot="job-progress"]')?.dataset["state"] ?? null,
              headline: card?.querySelector('[data-slot="job-progress"] span.truncate')?.textContent ?? null,
              detail: card ? [...card.querySelectorAll('[data-slot="job-detail"]')].map((line) => line.textContent) : [],
              buttons: card ? [...card.querySelectorAll("button")].map((button) => button.textContent) : [],
              timer: card?.querySelector("time") !== null && card?.querySelector("time") !== undefined,
              bar: card?.querySelector('[role="progressbar"]') ? [...card.querySelectorAll<HTMLElement>("[data-segment]")].map((segment) => segment.dataset["segment"]) : null,
              line: card ? [...card.querySelectorAll<HTMLElement>("[data-stage]")].map((stage) => `${stage.dataset["stage"]}:${stage.textContent}`) : [],
              spinning: card?.querySelector('[class*="animate-spin"]') !== null && card?.querySelector('[class*="animate-spin"]') !== undefined,
              status: card?.querySelector('[role="status"]')?.textContent ?? null,
              text: card?.textContent ?? "",
              schedule: column.querySelector('[data-slot="schedule-card"]')?.textContent ?? null,
            },
          ];
        }),
      );
      const tab = document.querySelector('[data-shot="tab"]')!;
      const steps = [...tab.querySelectorAll('[data-slot="timeline"] > li')];
      if (done) return;
      const result = document.createElement("pre");
      result.id = "job-card-result";
      result.textContent = JSON.stringify({
        ...seen,
        tab: {
          steps: steps.map((item) => item.querySelector("div > span")?.textContent),
          current: steps.map((item) => item.getAttribute("aria-current")),
        },
      });
      document.body.append(result);
    })();
    return () => {
      done = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="flex flex-wrap items-start gap-6 p-6">
        {Object.entries(MOMENTS).map(([id, parts]) => (
          <Session key={id} id={id} parts={parts} />
        ))}
        <Column id="scheduled">
          <ContextCard />
          <WaitCard title="Resumes at 4:00 AM" reason="when the Claude limit resets" atMs={NOW + 2 * 60 * MIN + 10 * MIN} nowMs={NOW} />
        </Column>
        <div data-shot="tab" className="bg-background w-160">
          <JobTimeline job={FAILED} />
        </div>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
