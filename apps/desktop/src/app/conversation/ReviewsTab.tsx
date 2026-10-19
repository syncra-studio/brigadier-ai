import { type ComponentProps, type ReactNode, useMemo } from "react";

import type { Merged } from "@/app/conversation/activity/revert";
import { CodeSpans } from "@/app/conversation/cards/codeSpans";
import { type FindingLabel, type FindingRow, useChecksLine, useReviewRow } from "@/app/conversation/reviewStatus";
import { Badge } from "@/components/ui/badge";
import type { MergeRecord } from "@/ipc/generated";
import { VENDOR_LABELS } from "@/lib/routing";
import { useBoard } from "@/state/board";

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

/**
 * The session's reviews and checks in full, in a tab of their own (opened from the context card's
 * Reviewed row or a merge's warning line): the full checks on the landed work, each review's
 * findings at full width with what the thread recorded of each, and what was checked of each
 * merged commit. Nothing here gates anything.
 */
export function ReviewsTab({ conversationId }: { conversationId: string }) {
  const row = useReviewRow(conversationId);
  const checks = useChecksLine(conversationId);
  const json = useBoard((s) =>
    s.board?.conversationId === conversationId
      ? JSON.stringify(s.board.orchestratorSteps.flatMap((step) => (step.kind.type === "merged" && step.kind.record ? [step.kind] : [])))
      : "[]",
  );
  const merges = useMemo(() => (JSON.parse(json) as Merged[]).toReversed(), [json]);
  const sections = row?.sections ?? [];
  const empty = !checks && sections.length === 0 && merges.length === 0;
  return (
    <div data-slot="reviews-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-6 px-5 pt-6 pb-10">
        {empty && <p className="text-muted-foreground text-sm">No reviews or checks yet.</p>}
        {checks && (
          <Part title="Checks">
            <p data-slot="reviews-checks" className={checks.failed ? "text-destructive text-sm" : "text-sm"}>
              {checks.text}
            </p>
            <p className="text-muted-foreground text-sm">{checks.title}</p>
          </Part>
        )}
        {sections.map((section, index) => (
          <Part key={section.label ?? index} title={`${section.label ?? "Review"} · ${section.outcome}`}>
            {section.findings.length > 0 && (
              <ul className="flex flex-col">
                {section.findings.map((finding) => (
                  <Finding key={finding.id} finding={finding} />
                ))}
              </ul>
            )}
          </Part>
        ))}
        {merges.map((merge) => (
          <Part key={merge.mergeCommit ?? merge.head ?? merge.branch} title={`Merge ${merge.mergeCommit ? `${merge.mergeCommit.slice(0, 7)} ` : ""}into ${merge.base}`}>
            <ul data-slot="reviews-merge-checked" className="flex flex-col gap-0.5 text-sm">
              {checkedLines(merge.record!, merge.pr !== undefined).map((line) => (
                <li key={line.text} data-warn={line.warn || undefined} className={line.warn ? "text-warning" : "text-muted-foreground"}>
                  {line.text}
                </li>
              ))}
            </ul>
          </Part>
        ))}
      </div>
    </div>
  );
}

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

function Part({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section data-slot="reviews-part" className="flex flex-col gap-2">
      <h3 className="text-muted-foreground text-label">{title}</h3>
      {children}
    </section>
  );
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
