/** A standalone Vite entry, excluded from the app entry and production build. Synthetic data. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { type ReactNode, useEffect, useMemo } from "react";
import { createRoot } from "react-dom/client";
import { createStore } from "zustand/vanilla";

import { StackCard, WaitCard } from "@/app/conversation/cards/JobCard";
import { type Job } from "@/app/conversation/job";
import { JobStack } from "@/app/conversation/JobStack";
import { JobTimeline } from "@/app/side-panel/JobTab";
import { SummaryRow, SummarySection } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick } from "@/fixtures/input";
import type { CiRun, IntegratedCheck, IntegratedCheckState, OrchestratorStep, Plan, Preview, Question, Request, ReviewRun, ReviewState, Task, UserRequest } from "@/ipc/generated";
import { type Board, BoardStoreContext, emptyBoard, type ShownApproval } from "@/state/board";

// Each column is the summary's column of cards (PinnedSummary's STACK) for a session of its own,
// its board holding the events of one moment. Times are a few minutes back from now.
const NOW = Date.now();
const MIN = 60_000;
const T0 = NOW - 20 * MIN;

/** What the Job card's buttons sent. */
const sent: string[] = [];
mockIPC((command, payload) => {
  if (command === "open_url") sent.push(`open ${(payload as { url: string }).url}`);
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  sent.push(req.method);
  return req.method === "cancelMerge" ? { method: req.method, text: "Cancelled" } : { method: req.method };
});

const request = (id: string, startedAtMs: number, state: UserRequest["state"], endedAtMs: number | null = null): UserRequest =>
  ({ id, conversationId: "c", preview: "", state, startedAtMs, endedAtMs, steeredInto: null, steeredAfter: null, undo: null, worked: [] }) as unknown as UserRequest;
const check = (n: number, tip: string, state: IntegratedCheckState, startedAtMs: number, endedAtMs: number | null = null): IntegratedCheck =>
  ({ id: `check-${n}`, conversationId: "c", repo: "/r", branch: "b", tip, state, dueAtMs: startedAtMs, startedAtMs, endedAtMs, requests: ["r1"], tasks: [], rerun: false });
const review = (state: ReviewState, startedAtMs: number, endedAtMs: number | null = null): ReviewRun =>
  ({ id: "v1", conversationId: "c", requestId: "r1", taskId: null, kind: "code", notify: { type: "orchestrator" }, base: "aaaaaaa000", tip: "4e1c49e999", author: "claude", reviewer: "codex", state, startedAtMs, endedAtMs, number: 1, items: [] }) as unknown as ReviewRun;
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
const held = step(T0 + 2 * MIN, { type: "mergeHeld", branch: "brigadier/s", base: "main", askedIn: "r1", consentedAtMs: T0 + 2 * MIN, condition: "workersFinish", waiting: ["t1"], tip: "abc" });
const passedOn = (tip: string) => byId([check(1, tip, { type: "passed", commands: ["a", "b", "c", "d", "e"] }, T0 + 3 * MIN, T0 + 8 * MIN)]);
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
  heldMerge: { ...working, tasks: byId([task("t1", "running")]), orchestratorSteps: [held] },
  ciFailed: { ...ended, orchestratorSteps: [ci(T0 + 3 * MIN, [ciRun("test", "completed", "failure", 5, 5)], "failed")] },
  idle: ended,
  working: { ...working, tasks: byId([task("t1", "running")]) },
  checksRunning: { ...working, integratedChecks: byId([check(1, "4e1c49e999", { type: "running", command: "desktop tests" }, T0 + 3 * MIN)]) },
  checksFailed: { ...working, integratedChecks: byId([check(1, "4e1c49e999", { type: "failed", command: "pnpm test", status: "2 tests failed", output: null }, T0 + 3 * MIN, T0 + 6 * MIN)]) },
  checksPassed: { ...ended, integratedChecks: passedOn("4e1c49e999") },
  reviewRunning: { ...working, reviews: byId([review({ type: "running" }, T0 + 3 * MIN)]) },
  reviewFindings: { ...ended, reviews: byId([review({ type: "findings", count: 3 }, T0 + 3 * MIN, T0 + 6 * MIN)]) },
  reviewClean: { ...ended, reviews: byId([review({ type: "clean" }, T0 + 3 * MIN, T0 + 6 * MIN)]) },
  ciChecking: { ...working, orchestratorSteps: [ci(T0 + 3 * MIN, [ciRun("test", "in_progress", "", 3, 5)])] },
  merging: { ...working, integratedChecks: passedOn("head111"), orchestratorSteps: [step(T0 + 9 * MIN, { type: "mergeRequested", id: "m1", branch: "brigadier/s", base: "main", origin: {} })] },
  merged: { ...ended, integratedChecks: passedOn("head111"), orchestratorSteps: [merged("head111")] },
  mergedUnchecked: { ...ended, integratedChecks: passedOn("other22"), orchestratorSteps: [merged("head111")] },
  waitingAnswer: { ...working, questions: { q1: question } },
  waitingApproval: { ...working, approvals: { a1: { id: "a1", state: { type: "pending" }, createdAtMs: T0 + 5 * MIN, subject: { type: "cli" } } as unknown as ShownApproval } },
  workerFailed: { ...working, tasks: byId([task("t1", "failed", { error: "The registry tests failed" })]) },
  devServer: {
    ...working,
    previews: byId([{ id: "p1", name: "The dev server", state: { type: "exited", code: 1, status: "Port 1420 is already in use" }, startedAtMs: T0 + MIN, endedAtMs: T0 + 2 * MIN } as unknown as Preview]),
  },
  stopped: { requests: byId([request("r1", T0, { type: "stopped" }, NOW - 2 * MIN)]) },
  // The user wrote since the checks passed: the outcome is gone with the turn.
  nextMessage: { requests: byId([request("r1", T0, { type: "done" }, T0 + 9 * MIN), request("r2", T0 + 10 * MIN, { type: "done" }, T0 + 11 * MIN)]), integratedChecks: passedOn("4e1c49e999") },
  headline: {
    ...working,
    tasks: byId([task("t1", "running")]),
    orchestratorSteps: [held],
    headline: { text: "Investigating the dev server", detail: "Looking for what holds port 1420", atMs: T0 + 6 * MIN },
  },
  plan: { ...working, tasks: byId([task("t1", "running")]), plans: { p1: plan } },
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
    { id: "review", name: "Reviewing", state: "done", startedAtMs: NOW - 20 * MIN, summary: "Review found 3 issues, all fixed" },
    { id: "checks", name: "Checks", state: "failed", startedAtMs: NOW - 9 * MIN, summary: "2 tests failed in the desktop app" },
    { id: "fix", name: "Fixing", state: "active", startedAtMs: NOW - 4 * MIN },
    { id: "merge", name: "Merging", state: "pending" },
  ],
};

const FINDINGS = (
  <ul className="text-muted-foreground flex list-disc flex-col gap-0.5 ps-4 text-xs">
    <li>The sidebar ignores the saved width after a restart.</li>
    <li>A worker's name can wrap under its avatar.</li>
    <li>The tab bar's close button has no label.</li>
  </ul>
);

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
      // The held merge's Cancel and CI's Open CI do what the rail's pills did.
      const cancel = document.querySelector('[data-column="heldMerge"] [data-slot="merge-cancel"]');
      if (cancel) await fixtureClick(cancel);
      const openCi = document.querySelector('[data-column="ciFailed"] [data-slot="job-open-ci"]');
      if (openCi) await fixtureClick(openCi);
      await sleep(100);
      const tab = document.querySelector('[data-shot="tab"]')!;
      const steps = [...tab.querySelectorAll('[data-slot="timeline"] > li')];
      if (done) return;
      const result = document.createElement("pre");
      result.id = "job-card-result";
      result.textContent = JSON.stringify({
        ...seen,
        sent,
        tab: {
          steps: steps.map((item) => item.querySelector("div > span")?.textContent),
          current: steps.map((item) => item.getAttribute("aria-current")),
          findings: tab.querySelectorAll('[data-slot="timeline"] ul > li').length,
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
          <JobTimeline job={FAILED} details={{ review: FINDINGS }} />
        </div>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
