import type { ComponentProps, ReactNode } from "react";

import { type BoardJob, useSessionJobs } from "@/app/conversation/boardJob";
import { CodeSpans } from "@/app/conversation/cards/codeSpans";
import { type Job, type JobStep, phaseWords } from "@/app/conversation/job";
import type { FindingLabel, FindingRow } from "@/app/conversation/reviewStatus";
import { Timeline, type TimelineEvent } from "@/components/assistant-ui/elements/timeline";
import { Badge } from "@/components/ui/badge";
import type { MergeRecord } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { VENDOR_LABELS } from "@/lib/routing";

/** A step's line under its name: what happened, or that it comes next. */
function stepDetail(step: JobStep, next: boolean): string | undefined {
  if (step.summary) return step.summary;
  if (step.state === "active") return step.count;
  return next ? "Next" : undefined;
}

/**
 * The Job card's tab: every step of the job on a timeline (Q88), each with when it ran, what
 * happened and what comes next; under a step, what it holds in full (`details`: a review's
 * findings, the checks of a merge).
 */
export function JobTab({ job, details }: { job: Job; details?: Readonly<Record<string, ReactNode>> }) {
  return (
    <TabPage>
      <JobTimeline job={job} details={details} />
    </TabPage>
  );
}

function TabPage({ children }: { children: ReactNode }) {
  return (
    <div data-slot="job-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-8 px-5 pt-6 pb-10">{children}</div>
    </div>
  );
}

/** One job's steps on a timeline, under its phase when it is a plan's. */
function JobTimeline({ job, title, details }: { job: Job; title?: ReactNode; details?: Readonly<Record<string, ReactNode>> | undefined }) {
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

/** Under a job's steps, what the board holds of them: the review's findings, the merge's record. */
function jobDetails(job: BoardJob): Record<string, ReactNode> {
  const { findings, merge } = job.facts;
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
    ...(merge?.record && {
      merge: (
        <ul data-slot="job-merge-checked" className="flex flex-col gap-0.5 pt-1 text-xs">
          {checkedLines(merge.record, merge.pr).map((line) => (
            <li key={line.text} data-warn={line.warn || undefined} className={line.warn ? "text-warning" : "text-muted-foreground"}>
              {line.text}
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
 * width and what was checked of each merged commit.
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

const short = (sha: string) => sha.slice(0, 7);

/**
 * What was checked of a merged commit, as recorded when it merged: each check, review, CI result
 * and verifier verdict, one plain line each: a check on an older
 * commit, a failure or something missing is marked `warn`.
 */
function checkedLines(record: MergeRecord, pr: boolean): { text: string; warn: boolean }[] {
  const head = short(record.head);
  const on = (sha: string, onHead: boolean) => (onHead ? `on ${head}` : `on ${short(sha)}, not the merged commit`);
  const lines: { text: string; warn: boolean }[] = [];
  for (const check of record.checks) {
    const passed = check.status === "exit 0";
    lines.push({ text: `${check.command}: ${passed ? "passed" : check.status} ${check.onHead ? `on ${head}` : "on an earlier version"}`, warn: !passed || !check.onHead });
  }
  if (record.checks.length === 0) lines.push({ text: "No tests or type checks ran", warn: true });
  for (const review of record.reviews) {
    const findings = review.fixed + review.dismissed + review.open;
    const outcome =
      review.state.type === "clean"
        ? "clean"
        : review.state.type === "running"
          ? "still running"
          : review.state.type === "failed"
            ? "couldn't run"
            : `${findings} finding${findings === 1 ? "" : "s"}, ${review.fixed} fixed, ${review.dismissed} dismissed, ${review.open} open`;
    lines.push({
      text: `${VENDOR_LABELS[review.reviewer]} review of ${short(review.from)}..${short(review.to)}: ${outcome}${review.onHead ? "" : ", not up to the merged commit"}`,
      warn: !review.onHead || review.open > 0 || review.state.type === "running" || review.state.type === "failed",
    });
  }
  if (record.reviews.length === 0) lines.push({ text: "No review", warn: true });
  if (record.ci) {
    const result = record.ci.result ?? "still running";
    lines.push({ text: `CI ${result} ${on(record.ci.sha, record.ci.onHead)}`, warn: result !== "passed" || !record.ci.onHead });
  } else if (pr) {
    lines.push({ text: "No CI result", warn: true });
  }
  if (record.verifier) {
    const result = record.verifier.result;
    const said = result === null ? "no report yet" : { passed: "passed", failed: "failed", notRun: "couldn't run every check", noChecks: "no checks to run" }[result];
    lines.push({ text: `Fresh verifier: ${said} ${on(record.verifier.commit, record.verifier.onHead)}`, warn: result !== "passed" || !record.verifier.onHead });
  }
  lines.push({ text: record.proofs === 0 ? "No screenshot or recording" : `${record.proofs} screenshot${record.proofs === 1 ? "" : "s"} or recording${record.proofs === 1 ? "" : "s"}`, warn: false });
  return lines;
}

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

