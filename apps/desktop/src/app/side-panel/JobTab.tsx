import type { ReactNode } from "react";

import { useSessionJobs } from "@/app/conversation/boardJob";
import { type Job, type JobStep, phaseWords } from "@/app/conversation/job";
import { overnightActions, useOvernightCards } from "@/app/conversation/overnightAdapter";
import { Timeline, type TimelineEvent } from "@/components/conversation";
import { RunPart } from "@/app/side-panel/RunPart";
import { TabPage } from "@/app/side-panel/TabPage";
import { formatTime } from "@/lib/format";

/** A step's line under its name: what happened, or that it comes next. */
function stepDetail(step: JobStep, next: boolean): string | undefined {
  if (step.summary) return step.summary;
  if (step.state === "active") return step.count;
  return next ? "Next" : undefined;
}

/**
 * One job's steps on a timeline (Q88), under its phase when it is a plan's: each with when it
 * ran, what happened and what comes next.
 */
export function JobTimeline({ job, title }: { job: Job; title?: ReactNode }) {
  const next = job.steps.find((step) => step.state === "pending");
  const events: TimelineEvent[] = job.steps.map((step) => {
    const at = step.startedAtMs ?? step.endedAtMs;
    return {
      id: step.id,
      when: step.state === "pending" ? "future" : step.state === "active" ? "now" : "past",
      time: at ? formatTime(at) : "",
      title: step.state === "failed" ? (step.failedLabel ?? `${step.name} failed`) : step.name,
      detail: stepDetail(step, step === next),
      failed: step.state === "failed",
    };
  });
  const phase = phaseWords(job);
  const heading = phase ? <>{phase} · <span className="text-foreground">{job.phase?.title}</span></> : title;
  return (
    <section data-slot="job-part" className="flex flex-col gap-4">
      {heading && <h2 className="text-muted-foreground text-label">{heading}</h2>}
      <Timeline events={events} />
    </section>
  );
}

/**
 * The session's job tab, opened from the Job card or a merge's warning line: its overnight runs,
 * newest first (each with its plan, its phases and its report), then the job at work, step by
 * step, then each earlier merge's, newest first.
 */
export function SessionJobTab({ conversationId }: { conversationId: string }) {
  const jobs = useSessionJobs(conversationId);
  const runs = useOvernightCards(conversationId);
  const past = jobs?.past ?? [];
  return (
    <TabPage>
      {!jobs?.current && past.length === 0 && runs.length === 0 && <p className="text-muted-foreground text-sm">Nothing has run yet.</p>}
      {runs.toReversed().map((model) => (
        <RunPart key={model.run.id} model={model} actions={overnightActions} />
      ))}
      {jobs?.current && <JobTimeline job={jobs.current} title="Now" />}
      {past.map((job) => (
        <JobTimeline key={job.atMs} job={job} title={job.steps.at(-1)?.summary ?? "Merged"} />
      ))}
    </TabPage>
  );
}
