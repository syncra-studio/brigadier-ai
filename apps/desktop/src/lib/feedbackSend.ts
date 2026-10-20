/**
 * Send feedback's engine (state/feedback.ts is how the app calls it): the note is written up as
 * reports by the daemon's writer, then filed as the user with their `gh`, one by one (a repeat
 * as a comment on its issue). Signed out, each report's form opens in the browser instead,
 * filled in. With no writer to run, the note opens in one form, as it used to. The thread's
 * card writes the note up first, to show the reports, and files them on Send.
 */
import { openUrl, request } from "@/ipc/client";
import type { FeedbackFiled, FeedbackFiling, FeedbackReport, FeedbackWriteUp } from "@/ipc/generated";
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
  /** Signed out: it repeats an issue, so nothing opened; the issue is where to look. */
  | { outcome: "known"; number: number; url: string }
  /** Its form opened in the browser, filled in. */
  | { outcome: "opened" }
  /** Not filed; `retryFeedback` sends it again. */
  | { outcome: "failed"; reason: string; report: FeedbackReport; searched: boolean }
);

/** What sending is at: writing the note up (and looking for repeats), or filing `count` reports. */
export type FeedbackStep = { step: "writing" } | { step: "filing"; count: number };

/** The note `text` as reports, checked against the issues; `null` when no writer could run. */
export async function writeUp(text: string): Promise<FeedbackWriteUp | null> {
  try {
    return (await request({ method: "writeUpFeedback", text })).writeUp;
  } catch (cause) {
    console.warn("feedback: writing it up failed", cause);
    return null;
  }
}

/**
 * Files what `writeUp` made of `text`. With no write-up (no writer could run) the note opens
 * in one form, as it did before reports were written: the `kind` given, else a bug's.
 */
export async function fileWriteUp(
  details: FeedbackDetails,
  text: string,
  kind: FeedbackKind | undefined,
  written: FeedbackWriteUp | null,
): Promise<FeedbackResult[]> {
  if (!written || written.reports.length === 0) {
    const fallback = kind ?? "bug";
    await openUrl(feedbackUrl(fallback, text, details));
    return [{ kind: fallback, title: feedbackTitle(text), outcome: "opened" }];
  }
  return file(written.reports, written.searched, details);
}

/** Sends `text` with `details`: written up as reports, then filed; what became of each. */
export async function send(
  details: FeedbackDetails,
  text: string,
  kind?: FeedbackKind,
  onStep?: (step: FeedbackStep) => void,
): Promise<FeedbackResult[]> {
  onStep?.({ step: "writing" });
  const written = await writeUp(text);
  onStep?.({ step: "filing", count: written?.reports.length || 1 });
  return fileWriteUp(details, text, kind, written);
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
    for (const report of reports) {
      const head = { kind: report.kind, title: report.title };
      if (report.duplicate) {
        const { number, url } = report.duplicate;
        shown.push({ ...head, outcome: "known", number, url });
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
