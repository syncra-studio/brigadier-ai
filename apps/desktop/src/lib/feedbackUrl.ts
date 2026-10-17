/**
 * The link Send feedback opens: a new issue in Brigadier's repository, filled in from the
 * repository's own issue forms (state/feedback.ts opens it).
 */

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

/** The details, as the dialog shows them and the issue carries them ("Brigadier 0.1.0, macOS 27.0 (arm64)"). */
export function detailsLine({ version, system, agents }: FeedbackDetails): string {
  return [version ? `Brigadier ${version}` : null, system, ...agents].filter(Boolean).join(", ");
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
