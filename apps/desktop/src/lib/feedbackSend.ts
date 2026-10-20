/**
 * Send feedback's engine (state/feedback.ts is how the app calls it): the note is written up as
 * reports by the daemon's writer, then filed as the user with their `gh`, one by one (a repeat
 * as a comment on its issue). Signed out, each report's form opens in the browser instead,
 * filled in. With no writer to run, the note opens in one form, as it used to.
 */
import { openUrl, request } from "@/ipc/client";
import type { FeedbackFiled, FeedbackFiling, FeedbackReport } from "@/ipc/generated";
import {
  type FeedbackDetails,
  type FeedbackKind,
  feedbackTitle,
  feedbackUrl,
  repeatComment,
  reportBody,
  reportTitle,
  reportUrl,
} from "@/lib/feedbackUrl";

/** What became of one report. */
export type FeedbackResult = { kind: FeedbackKind; title: string } & (
  /** Filed as a new issue. */
  | { outcome: "filed"; number: number; url: string }
  /** Added to the issue it repeats, as a comment. */
  | { outcome: "added"; number: number; url: string }
  /** Signed out: it repeats an issue, and its comment wasn't sent (the user may copy it). */
  | { outcome: "known"; number: number; url: string; comment: string }
  /** Its form opened in the browser, filled in. */
  | { outcome: "opened" }
  /** Not filed; `retryFeedback` sends it again. */
  | { outcome: "failed"; reason: string; report: FeedbackReport; searched: boolean }
);

/** Sends `text` with `details`: written up as reports, then filed; what became of each. */
export async function send(details: FeedbackDetails, text: string, kind?: FeedbackKind): Promise<FeedbackResult[]> {
  let writeUp;
  try {
    ({ writeUp } = await request({ method: "writeUpFeedback", text }));
  } catch (cause) {
    // No writer could run: the note goes into one form, as it did before reports were written.
    console.warn("feedback: writing it up failed; opening one form", cause);
    const fallback = kind ?? "bug";
    await openUrl(feedbackUrl(fallback, text, details));
    return [{ kind: fallback, title: feedbackTitle(text), outcome: "opened" }];
  }
  return file(writeUp.reports, writeUp.searched, details);
}

/** Sends the reports that failed again; the others stay as they were. */
export async function retry(details: FeedbackDetails, results: readonly FeedbackResult[]): Promise<FeedbackResult[]> {
  const failed = results.flatMap((result) => (result.outcome === "failed" ? [result] : []));
  if (failed.length === 0) return [...results];
  const byReport = new Map<FeedbackReport, FeedbackResult>();
  for (const searched of [true, false]) {
    const reports = failed.filter((result) => result.searched === searched).map((result) => result.report);
    if (reports.length === 0) continue;
    const again = await file(reports, searched, details);
    reports.forEach((report, index) => byReport.set(report, again[index]!));
  }
  return results.map((result) => (result.outcome === "failed" ? (byReport.get(result.report) ?? result) : result));
}

/** Files `reports` with the user's `gh`, else opens each in the browser. */
async function file(reports: FeedbackReport[], searched: boolean, details: FeedbackDetails): Promise<FeedbackResult[]> {
  const items: FeedbackFiling[] = reports.map((report) =>
    report.duplicate
      ? { kind: report.kind, title: "", body: repeatComment(report, details), commentOn: report.duplicate.number }
      : { kind: report.kind, title: reportTitle(report), body: reportBody(report, details, searched), commentOn: null },
  );
  const { signedIn, results } = await request({ method: "fileFeedback", items });
  if (!signedIn) {
    const shown: FeedbackResult[] = [];
    for (const [index, report] of reports.entries()) {
      const head = { kind: report.kind, title: report.title };
      if (report.duplicate) {
        const { number, url } = report.duplicate;
        shown.push({ ...head, outcome: "known", number, url, comment: items[index]!.body });
      } else {
        await openUrl(reportUrl(report, details));
        shown.push({ ...head, outcome: "opened" });
      }
    }
    return shown;
  }
  return reports.map((report, index) => resultOf(report, searched, results[index]));
}

function resultOf(report: FeedbackReport, searched: boolean, filed: FeedbackFiled | undefined): FeedbackResult {
  const head = { kind: report.kind, title: report.title };
  switch (filed?.outcome) {
    case "filed":
      return { ...head, outcome: "filed", number: filed.number, url: filed.url };
    case "commented":
      return { ...head, outcome: "added", number: filed.number, url: filed.url };
    case "failed":
      return { ...head, outcome: "failed", reason: filed.reason, report, searched };
    default:
      return { ...head, outcome: "failed", reason: "GitHub said nothing about it", report, searched };
  }
}
