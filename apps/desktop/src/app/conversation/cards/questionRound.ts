import type { Question, QuestionItem } from "@/ipc/generated";

/** A card's questions: its round, or the single question it asks (as every card before rounds). */
export function questionRound(question: Question): QuestionItem[] {
  if (question.items.length > 0) return question.items;
  return [
    {
      text: question.text,
      options: question.options.map((label) => ({ label, description: null })),
      recommended: question.recommended,
    },
  ];
}

/**
 * The uncommitted-changes card's title and files: the user's own, then those the thread's
 * edits left, marked as the thread's and never as the user's (#88). Null for another card.
 */
export function uncommittedCard(
  question: Question,
): { title: string; files: { path: string; threads: boolean }[] } | null {
  if (question.kind.type !== "uncommittedChanges") return null;
  const { files, threadFiles = [] } = question.kind;
  return {
    title:
      files.length === 0 && threadFiles.length > 0
        ? "Should workers see the thread's uncommitted changes?"
        : "Should workers see your uncommitted changes?",
    files: [
      ...files.map((path) => ({ path, threads: false })),
      ...threadFiles.map((path) => ({ path, threads: true })),
    ],
  };
}

/** The answers given, one per question; empty while it waits or once withdrawn. */
export function questionAnswers(question: Question): string[] {
  if (question.answers.length > 0) return question.answers;
  return question.answer === null ? [] : [question.answer];
}

/** Whether the card still waits for the user. */
export function questionOpen(question: Question): boolean {
  return question.answer === null && question.answeredAtMs === null;
}

/**
 * An answer as the thread shows it: the recommended option says so. It shows as plain text, so
 * Markdown's backslash escapes in text pasted from a Markdown source ("1\. Cover") go.
 */
export function answerWords(item: QuestionItem, answer: string): string {
  if (!answer.trim()) return "No answer provided";
  const recommended = item.recommended === null ? undefined : item.options[item.recommended];
  const mark = (label: string) => (recommended?.label === label ? `${label} (Recommended)` : label);
  // A multi-select answer, each option marked as one would be.
  const picks = item.multiSelect ? multiPicks(item, answer) : null;
  if (picks) return picks.map((index) => mark(item.options[index]!.label)).join(", ");
  return recommended?.label === answer ? mark(answer) : answer.replace(/\\([!-/:-@[-`{-~])/g, "$1");
}

/**
 * A multi-select answer's options, by index, as the card joins them ("red, blue"); `null` when
 * it is the user's own words. An option whose label holds ", " is one pick.
 */
export function multiPicks(item: QuestionItem, answer: string): number[] | null {
  const index = (label: string) => item.options.findIndex((option) => option.label === label);
  if (index(answer) >= 0) return [index(answer)];
  const parts = answer.split(", ").map(index);
  return parts.length > 1 && parts.every((at) => at >= 0) ? parts : null;
}

/** Text with its `code` spans' backticks left out, for where it can't show code (a tooltip, a label). */
export function plainCode(text: string): string {
  return codeParts(text).map((part) => part.text).join("");
}

/**
 * The thread's row for a card: what it asks while open ("Asking questions · waiting"), how many it asked
 * once answered ("Asked 3 questions"), each question and its answer under the row.
 */
export function questionRowWords(question: Question): string {
  const words = askedWords(question);
  // Open, its one row says it waits too: no second "Waiting for your answer" line (D161).
  return questionOpen(question) ? `${words} · waiting` : words;
}

function askedWords(question: Question): string {
  const open = questionOpen(question);
  switch (question.kind.type) {
    case "merge":
      return open ? `Asking whether to merge into ${question.kind.base}` : `Asked whether to merge into ${question.kind.base}`;
    case "uncommittedChanges":
      return open ? "Asking about your uncommitted changes" : "Asked about your uncommitted changes";
    case "orchestrator": {
      const count = questionRound(question).length;
      if (open) return count === 1 ? "Asking a question" : "Asking questions";
      const asked = `Asked ${count} ${count === 1 ? "question" : "questions"}${question.title ? ` · ${question.title}` : ""}`;
      return question.answer === null ? `${asked} · withdrawn` : asked;
    }
  }
}

/** One piece of a card line: plain text, a `code` span, or a web link to `href`. */
export type CardPart = { text: string; code: boolean; href?: string };

/** `[label](url)` with a web address, or a bare web address. */
const LINK = /\[([^[\]\n]+)\]\((https?:\/\/[^\s()]+(?:\([^\s()]*\)[^\s()]*)*)\)|https?:\/\/[^\s<>"'`]+/gi;

/** A bare address without what follows it in the sentence: "see https://a.b/c)." is https://a.b/c. */
function bareAddress(found: string): string {
  let address = found;
  for (;;) {
    const last = address.at(-1) ?? "";
    const opener = { ")": "(", "]": "[", "}": "{" }[last];
    const unmatched = opener !== undefined && address.split(last).length > address.split(opener).length;
    if (!unmatched && !".,;:!?*".includes(last)) return address;
    address = address.slice(0, -1);
  }
}

function isWebAddress(address: string): boolean {
  try {
    return /^https?:$/.test(new URL(address).protocol);
  } catch {
    return false;
  }
}

function linkParts(text: string, exact: boolean): CardPart[] {
  const parts: CardPart[] = [];
  let from = 0;
  for (const { 0: found, 1: label, 2: labelled, index } of text.matchAll(LINK)) {
    const href = labelled ?? bareAddress(found);
    if (!isWebAddress(href)) continue;
    const end = index + (labelled ? found.length : href.length);
    parts.push({ text: text.slice(from, index), code: false });
    parts.push({ text: exact ? text.slice(index, end) : (label ?? href), code: false, href });
    from = end;
  }
  parts.push({ text: text.slice(from), code: false });
  return parts;
}

/**
 * A card line's parts: plain text, the `code` spans written in backticks (a branch, a file)
 * without the backticks, and web links, bare or `[label](url)`, the only links there are (no link
 * inside code). An unclosed backtick stays as written.
 *
 * `exact`: for text that must read character for character (a command asked about). Its backticks
 * and `[label](url)` keep their characters, the link's whole text clickable.
 */
export function codeParts(text: string, exact = false): CardPart[] {
  return text
    .split(/`([^`\n]+)`/)
    .flatMap((part, index) =>
      index % 2 === 1 ? [{ text: exact ? `\`${part}\`` : part, code: true }] : linkParts(part, exact),
    )
    .filter((part) => part.text !== "");
}

/** Whether the user's selection reaches into `element`: a drag over its words left it, or a double-click. */
export function selectedWithin(element: Element): boolean {
  const selection = element.ownerDocument.getSelection();
  return !!selection && !selection.isCollapsed && selection.containsNode(element, true);
}

/** What an answer's copy button copies: the `code` spans its words name (a command), one a line; none without. */
export function optionCode(...texts: readonly (string | null | undefined)[]): string | undefined {
  const code = texts.flatMap((text) => (text ? codeParts(text).filter((part) => part.code).map((part) => part.text) : []));
  return code.length > 0 ? code.join("\n") : undefined;
}
