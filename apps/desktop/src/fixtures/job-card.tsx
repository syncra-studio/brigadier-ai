import { type ReactNode, useEffect } from "react";
import { createRoot } from "react-dom/client";

import { JobCard, StackCard, WaitCard } from "@/app/conversation/cards/JobCard";
import { type Job, jobShown } from "@/app/conversation/job";
import { JobTimeline } from "@/app/conversation/JobTab";
import { SummaryRow, SummarySection } from "@/components/assistant-ui/elements/summary-section";

// Each column is the summary's column of cards (PinnedSummary's STACK), at a fixed clock.
const NOW = new Date(2026, 9, 19, 2, 0).getTime();
const MIN = 60_000;

const WORKING: Job = {
  steps: [
    { id: "build", name: "Building", state: "done", startedAtMs: NOW - 42 * MIN, summary: "Two workers built it" },
    { id: "review", name: "Reviewing", state: "active", count: "3 of 5", startedAtMs: NOW - 6 * MIN },
    { id: "fix", name: "Fixing", state: "pending" },
    { id: "checks", name: "Checks", state: "pending" },
    { id: "merge", name: "Merging", state: "pending" },
  ],
};

const FAILED: Job = {
  steps: [
    { id: "build", name: "Building", state: "done", startedAtMs: NOW - 50 * MIN, summary: "Two workers built it" },
    { id: "review", name: "Reviewing", state: "done", startedAtMs: NOW - 20 * MIN, summary: "Review found 3 issues, all fixed" },
    { id: "checks", name: "Checks", state: "failed", startedAtMs: NOW - 9 * MIN, summary: "2 tests failed in the desktop app" },
    { id: "fix", name: "Fixing", state: "active", startedAtMs: NOW - 4 * MIN },
    { id: "merge", name: "Merging", state: "pending" },
  ],
};

const PHASED: Job = {
  phase: { number: 2, of: 4, title: "Implement Windows paths" },
  steps: [
    { id: "p2-build", name: "Building", state: "done", startedAtMs: NOW - 30 * MIN },
    { id: "p2-review", name: "Reviewing", state: "active", startedAtMs: NOW - 3 * MIN },
    { id: "p2-checks", name: "Checks", state: "pending" },
  ],
};

const DONE: Job = { steps: [{ id: "merge", name: "Merging", state: "done", startedAtMs: NOW - MIN }] };

/** A stand-in for the context card: its surface and two of its rows. */
function ContextCard() {
  return (
    <StackCard className="pt-2 pb-2">
      <SummarySection foldKey="fixture-project" title="brigadier-ai">
        <SummaryRow meta={<span><span className="text-success">+380</span> <span className="text-destructive">−2</span></span>}>brigadier/web-ui</SummaryRow>
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

const show = (job: Job) => (jobShown(job) ? <JobCard job={job} onOpen={() => {}} /> : null);

const FINDINGS = (
  <ul className="text-muted-foreground flex list-disc flex-col gap-0.5 ps-4 text-xs">
    <li>The sidebar ignores the saved width after a restart.</li>
    <li>A worker's name can wrap under its avatar.</li>
    <li>The tab bar's close button has no label.</li>
  </ul>
);

function Fixture() {
  useEffect(() => {
    const columns = [...document.querySelectorAll<HTMLElement>("[data-column]")];
    const seen = Object.fromEntries(
      columns.map((column) => [
        column.dataset["column"],
        {
          cards: [...column.children].map((card) =>
            card.querySelector('[data-slot="job-card"]') ? "job" : card.querySelector('[data-slot="schedule-card"]') ? "schedule" : "context",
          ),
          label: column.querySelector('[data-slot="job-progress"] span.truncate')?.textContent ?? null,
          stages: [...column.querySelectorAll<HTMLElement>("[data-stage]")].map((stage) => stage.dataset["stage"]),
          names: [...column.querySelectorAll<HTMLElement>("[data-stage]")].map((stage) => stage.textContent),
          marks: [...column.querySelectorAll<HTMLElement>("[data-stage]")].map((stage) => stage.querySelector("[data-mark]")?.getAttribute("data-mark") ?? "cross"),
          shimmer: [...column.querySelectorAll<HTMLElement>("[data-stage] .shimmer")].map((name) => name.textContent),
          bar: column.querySelector('[data-slot="job-progress"] [role="progressbar"]')?.getAttribute("aria-valuenow") ?? null,
          red: column.querySelector('[data-slot="job-progress"] .text-destructive') !== null,
          schedule: column.querySelector('[data-slot="schedule-card"]')?.textContent ?? null,
        },
      ]),
    );
    const tab = document.querySelector('[data-shot="tab"]')!;
    const steps = [...tab.querySelectorAll('[data-slot="timeline"] > li')];
    const result = document.createElement("pre");
    result.id = "job-card-result";
    result.textContent = JSON.stringify({
      ...seen,
      tab: {
        steps: steps.map((item) => item.querySelector("div > span")?.textContent),
        current: steps.map((item) => item.getAttribute("aria-current")),
        findings: tab.querySelectorAll('[data-slot="timeline"] ul > li').length,
      },
    });
    document.body.append(result);
    return () => result.remove();
  }, []);
  return (
    <div className="flex flex-wrap items-start gap-6 p-6">
      <Column id="working">
        <ContextCard />
        {show(WORKING)}
      </Column>
      <Column id="failed">
        <ContextCard />
        {show(FAILED)}
      </Column>
      <Column id="phased">
        <ContextCard />
        {show(PHASED)}
      </Column>
      <Column id="scheduled">
        <ContextCard />
        <WaitCard title="Resumes at 4:00 AM" reason="when the Claude limit resets" atMs={NOW + 2 * 60 * MIN + 10 * MIN} nowMs={NOW} />
      </Column>
      {/* The context card hidden: the job and what waits take its place. */}
      <Column id="hidden">
        {show(WORKING)}
        <WaitCard title="The overnight run ends at 7:30 AM" atMs={NOW + 5.5 * 60 * MIN} nowMs={NOW} />
      </Column>
      <Column id="done">
        <ContextCard />
        {show(DONE)}
      </Column>
      <div data-shot="tab" className="bg-background w-160">
        <JobTimeline job={FAILED} details={{ review: FINDINGS }} />
      </div>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
