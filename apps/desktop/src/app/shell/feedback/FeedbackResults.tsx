import { useState } from "react";

import { Button } from "@/components/controls";
import { ArrowUpRight, Bug, Lightbulb } from "@/components/icons";
import { openUrl } from "@/ipc/client";
import type { FeedbackKind } from "@/lib/feedbackUrl";
import { cn } from "@/lib/utils";
import { type FeedbackAccount, type FeedbackResult, type FeedbackStep, feedbackDetails } from "@/state/feedback";

/** A report's kind as its icon: a bug, or an idea's bulb. */
export function KindIcon({ kind }: { kind: FeedbackKind }) {
  const Icon = kind === "bug" ? Bug : Lightbulb;
  return <Icon aria-label={kind === "bug" ? "Bug" : "Idea"} className="text-muted-foreground size-icon-md shrink-0" />;
}

/**
 * What Send feedback filed, one row per report in a soft box: its kind, its title, and where it
 * went (its new issue, the issue it was added to, the browser, or the issue it repeats). Shown by
 * the dialog and the thread's feedback card once sent.
 */
export function FeedbackResults({ results }: { results: readonly FeedbackResult[] }) {
  return (
    <ul aria-label="Sent" className="bg-foreground/5 rounded-settings flex flex-col">
      {results.map((result, index) => (
        <li
          key={`${index}-${result.title}`}
          className={cn("flex min-w-0 items-center gap-2.5 px-3 py-2", index > 0 && "border-divider border-t")}
        >
          <KindIcon kind={result.kind} />
          <span className="text-label min-w-0 flex-1 truncate">{result.title}</span>
          <Where result={result} />
        </li>
      ))}
    </ul>
  );
}

function Where({ result }: { result: FeedbackResult }) {
  switch (result.outcome) {
    case "filed":
      return <IssueLink label={`#${result.number}`} url={result.url} />;
    case "added":
      return <IssueLink label={`Added to #${result.number}`} url={result.url} />;
    case "known":
      return <IssueLink label={`Already reported: #${result.number}`} url={result.url} />;
    case "opened":
      return <span className="text-muted-foreground text-label shrink-0">Opened in your browser</span>;
    case "failed":
      return <span className="text-destructive text-label shrink-0">Not sent</span>;
  }
}

/** A link to an issue on GitHub, opened in the browser. */
function IssueLink({ label, url }: { label: string; url: string }) {
  return (
    <Button variant="link" size="inline" className="text-sm" onClick={() => void openUrl(url)}>
      {label}
      <ArrowUpRight />
    </Button>
  );
}

/** Whether a done send went to the browser (signed out, or no writer): nothing was posted. */
export function sentToBrowser(results: readonly FeedbackResult[]): boolean {
  return results.every((result) => result.outcome === "opened" || result.outcome === "known");
}

/** What a done send says under its title: how many, and where they went. */
export function sentLine(results: readonly FeedbackResult[]): string {
  const one = results.length === 1;
  if (sentToBrowser(results)) return one ? "Opened in your browser. Post it there." : `${results.length} reports opened in your browser. Post each one there.`;
  return one ? "Your report, filed on GitHub." : `${results.length} reports, filed on GitHub.`;
}

/** What a send that filed only some says under its title (with Try again for the rest). */
export function unsentLine(unsent: number): string {
  return `${unsent === 1 ? "One report" : `${unsent} reports`} couldn’t be sent.`;
}

/** What a step says, and how far along its bar is (writing up, then filing, of three). */
export function stepProgress(
  step: FeedbackStep,
  account: FeedbackAccount | null,
): { description: string; done: number; total: number } {
  if (step.step === "writing") {
    return { description: "Splitting your note and checking GitHub for ones already reported", done: 1, total: 3 };
  }
  const one = step.count === 1;
  const description =
    account?.signedIn === false
      ? one
        ? "Opening it in your browser"
        : "Opening each one in your browser"
      : one
        ? "Filing it on GitHub"
        : `Filing ${step.count} reports on GitHub`;
  return { description, done: 2, total: 3 };
}

/** The system's name in a word ("macOS"), as "with your app and macOS version" says it. */
export function useSystemWord(): string {
  // Known since startup, so the words never change once shown (#470).
  const [word] = useState(() => feedbackDetails().system?.split(" ")[0] || "system");
  return word;
}
