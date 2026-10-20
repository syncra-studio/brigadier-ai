/**
 * What Send feedback files or opens in Brigadier's repository, in the shape of its own issue
 * forms (state/feedback.ts sends it): each report's issue body, its form filled in for the
 * browser, a repeat's comment, and the one prefilled form used when no writer could run.
 */

import type { FeedbackReport } from "@/ipc/generated";

export type FeedbackKind = "bug" | "idea";

/** What goes with the text: this app's version, the system, and the agent CLIs found. */
export type FeedbackDetails = { version: string | null; system: string | null; agents: readonly string[] };

export const FEEDBACK_REPO = "https://github.com/syncra-studio/brigadier-ai";

/** The longest link it opens: past it the text is cut, so GitHub still takes the link. */
export const MAX_URL = 7500;
/** The issue's title: the text's first line, cut at this many characters. */
const MAX_TITLE = 72;

/** The text's first line, its spaces collapsed, cut to a title's length. */
export function feedbackTitle(text: string): string {
  const line = (text.split("\n").find((part) => part.trim()) ?? "").replace(/\s+/g, " ").trim();
  return line.length <= MAX_TITLE ? line : `${line.slice(0, MAX_TITLE - 1).trimEnd()}…`;
}

/** The details one by one, as the dialog's chips show them ("Brigadier 0.1.0", "macOS 27.0 (arm64)"). */
export function detailsItems({ version, system, agents }: FeedbackDetails): string[] {
  return [version ? `Brigadier ${version}` : null, system, ...agents].filter((item): item is string => Boolean(item));
}

/** The details on one line ("Brigadier 0.1.0, macOS 27.0 (arm64)"). */
export function detailsLine(details: FeedbackDetails): string {
  return detailsItems(details).join(", ");
}

/**
 * The new issue's address: the bug form (`bug_report.yml`) with what happened and the versions,
 * or the proposal form (`proposal.yml`) with the problem. The forms' own title prefixes stay.
 * Text too long for a link is cut (by whole characters) until the encoded link fits `MAX_URL`.
 */
export function feedbackUrl(kind: FeedbackKind, text: string, details: FeedbackDetails): string {
  const whole = text.trim();
  const url = withBody(kind, whole, details);
  if (url.length <= MAX_URL) return url;
  const characters = Array.from(whole);
  let fits = 0;
  let over = characters.length;
  while (over - fits > 1) {
    const middle = Math.floor((fits + over) / 2);
    if (withBody(kind, `${characters.slice(0, middle).join("")}…`, details).length <= MAX_URL) fits = middle;
    else over = middle;
  }
  return withBody(kind, `${characters.slice(0, fits).join("").trimEnd()}…`, details);
}

function withBody(kind: FeedbackKind, body: string, details: FeedbackDetails): string {
  const params = new URLSearchParams();
  if (kind === "bug") {
    params.set("template", "bug_report.yml");
    params.set("title", `[Bug]: ${feedbackTitle(body)}`);
    params.set("actual", body);
    if (details.version) params.set("version", details.version);
    const environment = [details.system, ...details.agents].filter(Boolean).join(", ");
    if (environment) params.set("environment", environment);
  } else {
    params.set("template", "proposal.yml");
    params.set("title", `[Proposal]: ${feedbackTitle(body)}`);
    params.set("problem", body);
  }
  return `${FEEDBACK_REPO}/issues/new?${params.toString()}`;
}

/** The blank new-issue page, where the user picks a form themselves. */
export const NEW_ISSUE_CHOOSER = `${FEEDBACK_REPO}/issues/new/choose`;

/** What a report's issue says about itself, first: where it came from, and the duplicate search. */
export function filedFrom(searched: boolean): string {
  return searched
    ? "_Filed from Brigadier’s Send feedback, written up from the reporter’s note. Brigadier searched open and closed issues for a duplicate first._"
    : "_Filed from Brigadier’s Send feedback, written up from the reporter’s note. Brigadier couldn’t search the issues for a duplicate._";
}

/** The form's AI assistance answer: Brigadier wrote it up. */
const AI_NOTE = "Written up by Brigadier from the reporter’s own words.";

/** A report's issue title, with its form's prefix. */
export function reportTitle(report: FeedbackReport): string {
  return `${report.kind === "bug" ? "[Bug]" : "[Proposal]"}: ${report.title}`;
}

/** The environment field: the system and the agent CLIs found. */
function environmentOf(details: FeedbackDetails): string {
  return [details.system, ...details.agents].filter(Boolean).join(", ");
}

/** A report's fields as its form names them (the form's ids, and the headings GitHub shows). */
function reportFields(report: FeedbackReport, details: FeedbackDetails): { id: string; label: string; value: string }[] {
  if (report.kind === "bug") {
    return [
      { id: "version", label: "Version or commit", value: details.version ?? "" },
      { id: "environment", label: "Environment", value: environmentOf(details) },
      { id: "area", label: "Area", value: report.area },
      { id: "steps", label: "Steps to reproduce", value: report.steps },
      { id: "expected", label: "Expected", value: report.expected },
      { id: "actual", label: "What happened", value: report.actual },
      { id: "ai", label: "AI assistance", value: AI_NOTE },
    ];
  }
  return [
    { id: "problem", label: "The problem", value: report.problem },
    { id: "today", label: "What you do today", value: report.today },
    { id: "wanted", label: "What should happen instead", value: report.wanted },
  ];
}

/**
 * A report's issue body, as its form would render it: the note on where it came from, then
 * each field under its heading ("_No response_" when empty, as GitHub writes it). The forms'
 * "Before you file" boxes are left out: the reporter didn't tick them.
 */
export function reportBody(report: FeedbackReport, details: FeedbackDetails, searched: boolean): string {
  const sections = reportFields(report, details).map(
    ({ label, value }) => `### ${label}\n\n${value.trim() || "_No response_"}`,
  );
  return [filedFrom(searched), ...sections].join("\n\n");
}

/** The comment a repeat adds to the issue it repeats, with the versions it was seen on. */
export function repeatComment(report: FeedbackReport, details: FeedbackDetails): string {
  const note = report.duplicate?.comment.trim() || "Seen again.";
  const seenOn = detailsLine(details);
  return [
    "_Reported again from Brigadier’s Send feedback._",
    note,
    ...(seenOn ? [`Seen on: ${seenOn}`] : []),
  ].join("\n\n");
}

/**
 * A report's form in the browser, filled in, one Create away. Fields too long for a link are
 * cut, the longest first, until the link fits `MAX_URL`.
 */
export function reportUrl(report: FeedbackReport, details: FeedbackDetails): string {
  const fields = Object.fromEntries(reportFields(report, details).map(({ id, value }) => [id, value.trim()]));
  const link = () => {
    const params = new URLSearchParams();
    params.set("template", report.kind === "bug" ? "bug_report.yml" : "proposal.yml");
    params.set("title", reportTitle(report));
    for (const [id, value] of Object.entries(fields)) if (value) params.set(id, value);
    return `${FEEDBACK_REPO}/issues/new?${params.toString()}`;
  };
  let url = link();
  while (url.length > MAX_URL) {
    const [id, value] = Object.entries(fields).reduce((longest, entry) => (entry[1].length > longest[1].length ? entry : longest));
    if (!value) break;
    const characters = Array.from(value);
    // As many characters as the overflow takes at the field's own encoded width, one more for "…".
    const width = new URLSearchParams({ value }).toString().length / characters.length;
    const keep = Math.max(0, characters.length - Math.ceil((url.length - MAX_URL) / width) - 1);
    fields[id] = keep > 0 ? `${characters.slice(0, keep).join("").trimEnd()}…` : "";
    url = link();
  }
  return url;
}

/** A report as the thread's card shows it opened: its fields in a line each, as they'll be posted. */
export function reportLines(report: FeedbackReport): string[] {
  const fields =
    report.kind === "bug"
      ? [
          ["What happened", report.actual],
          ["Expected", report.expected],
          ["Steps", report.steps],
        ]
      : [
          ["The problem", report.problem],
          ["Today", report.today],
          ["Wanted", report.wanted],
        ];
  return fields.flatMap(([label, value]) => (value?.trim() ? [`${label}: ${value.trim()}`] : []));
}
