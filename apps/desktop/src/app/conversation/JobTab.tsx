import type { ComponentProps, ReactNode } from "react";

import { mergeProblems } from "@/app/conversation/activity/mergeChecks";
import { type BoardJob, useSessionJobs } from "@/app/conversation/boardJob";
import { CodeSpans } from "@/app/conversation/cards/codeSpans";
import { type Job, type JobStep, phaseWords } from "@/app/conversation/job";
import type { FindingLabel, FindingRow } from "@/app/conversation/reviewStatus";
import { Timeline, type TimelineEvent } from "@/components/conversation/elements/timeline";
import { Badge } from "@/components/controls/badge";
import { formatTime } from "@/lib/format";
import { VENDOR_LABELS } from "@/lib/routing";

/** A step's line under its name: what happened, or that it comes next. */
function stepDetail(step: JobStep, next: boolean): string | undefined {
  if (step.summary) return step.summary;
  if (step.state === "active") return step.count;
  return next ? "Next" : undefined;
}

function TabPage({ children }: { children: ReactNode }) {
  return (
    <div data-slot="job-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-8 px-5 pt-6 pb-10">{children}</div>
    </div>
  );
}

/**
 * One job's steps on a timeline (Q88), under its phase when it is a plan's: each with when it
 * ran, what happened and what comes next; under a step, what it holds in full (`details`: a
 * review's findings, the checks of a merge).
 */
export function JobTimeline({ job, title, details }: { job: Job; title?: ReactNode; details?: Readonly<Record<string, ReactNode>> | undefined }) {
  const next = job.steps.find((step) => step.state === "pending");
  const events: TimelineEvent[] = job.steps.map((step) => {
    const at = step.startedAtMs ?? step.endedAtMs;
    return {
      id: step.id,
      when: step.state === "pending" ? "future" : step.state === "active" ? "now" : "past",
      time: at ? formatTime(at) : "",
      title: step.state === "failed" ? (step.failedLabel ?? `${step.name} failed`) : step.name,
      detail: stepDetail(step, step === next),
      more: details?.[step.id],
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

/** Under a job's steps, what the board holds of them: the review's findings, what is wrong with the merged code. */
function jobDetails(job: BoardJob): Record<string, ReactNode> {
  const { findings, merge } = job.facts;
  const problems = merge?.record ? mergeProblems(merge.record, VENDOR_LABELS) : [];
  return {
    ...(findings.length > 0 && {
      review: (
        <ul className="flex flex-col pt-1">
          {findings.map((finding) => (
            <Finding key={finding.id} finding={finding} />
          ))}
        </ul>
      ),
    }),
    ...(problems.length > 0 && {
      merge: (
        <ul data-slot="job-merge-checked" className="flex flex-col gap-0.5 pt-1 text-xs">
          {problems.map((line) => (
            <li key={line} data-warn className="text-warning">
              {line}
            </li>
          ))}
        </ul>
      ),
    }),
  };
}

/**
 * The session's job tab, opened from the Job card or a merge's warning line: the job at work,
 * step by step, then each earlier merge's, newest first, with every review's findings at full
 * width and what is wrong with each merged commit, if anything.
 */
export function SessionJobTab({ conversationId }: { conversationId: string }) {
  const jobs = useSessionJobs(conversationId);
  const past = jobs?.past ?? [];
  return (
    <TabPage>
      {!jobs?.current && past.length === 0 && <p className="text-muted-foreground text-sm">Nothing has run yet.</p>}
      {jobs?.current && <JobTimeline job={jobs.current} title="Now" details={jobDetails(jobs.current)} />}
      {past.map((job) => (
        <JobTimeline
          key={job.atMs}
          job={job}
          title={job.steps.at(-1)?.summary ?? "Merged"}
          details={jobDetails(job)}
        />
      ))}
    </TabPage>
  );
}

/**
 * Each chip coloured as the app's badges are: open warns, fixed is done, a fix nobody checked
 * warns softly, dismissed is quiet.
 */
const STATUS_BADGE: Record<FindingLabel, ComponentProps<typeof Badge>> = {
  Open: { variant: "warning" },
  New: { variant: "secondary" },
  Fixed: { variant: "success" },
  Unverified: { variant: "outline", className: "border-warning/50 text-warning" },
  Dismissed: { variant: "outline", className: "border-divider text-foreground/65" },
};

/** One finding at full width: what the thread recorded and how severe, its whole title, where, and the thread's words on it. */
function Finding({ finding }: { finding: FindingRow }) {
  return (
    <li data-slot="reviews-finding" title={finding.id} className="border-divider flex min-w-0 items-start gap-2 border-b py-2 text-sm last:border-b-0">
      <Badge data-slot="finding-status" {...STATUS_BADGE[finding.status]}>{finding.status}</Badge>
      {finding.severity && (
        <Badge variant="outline" className="border-divider text-foreground/65">
          {finding.severity}
        </Badge>
      )}
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-foreground wrap-break-word">
          <CodeSpans text={finding.title} />
        </span>
        {finding.location && <span className="text-muted-foreground font-mono text-xs break-all">{finding.location}</span>}
        {finding.note && <span className="text-muted-foreground text-xs">{finding.note}</span>}
      </span>
    </li>
  );
}

