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

/** The answers given, one per question; empty while it waits or once withdrawn. */
export function questionAnswers(question: Question): string[] {
  if (question.answers.length > 0) return question.answers;
  return question.answer === null ? [] : [question.answer];
}

/** Whether the card still waits for the user. */
export function questionOpen(question: Question): boolean {
  return question.answer === null && question.answeredAtMs === null;
}

/** An answer as the thread shows it: the recommended option says so. */
export function answerWords(item: QuestionItem, answer: string): string {
  if (!answer.trim()) return "No answer provided";
  const recommended = item.recommended === null ? undefined : item.options[item.recommended];
  return recommended?.label === answer ? `${answer} (Recommended)` : answer;
}

/** A question in one line: its first line, cut after its first question mark. */
export function shortQuestion(text: string): string {
  const first = text.trim().split("\n")[0]?.trim() ?? "";
  return /^.*?\?(?=\s|$)/.exec(first)?.[0] ?? first;
}

/**
 * An answered question card's row: the first question in one line, how many more the round
 * asked, and the answer to the first. Null for a card the user hasn't answered (or that was
 * withdrawn), and for the merge and uncommitted-changes cards, which say what they asked.
 */
export function askedRow(question: Question): { asked: string; more: number; answer: string } | null {
  if (question.kind.type !== "orchestrator") return null;
  const [answer] = questionAnswers(question);
  const round = questionRound(question);
  if (answer === undefined || round.length === 0) return null;
  return { asked: shortQuestion(round[0]?.text ?? ""), more: round.length - 1, answer: answer.split("\n")[0] ?? answer };
}

/** Text with its `code` spans' backticks left out, for where it can't show code (a tooltip, a label). */
export function plainCode(text: string): string {
  return codeParts(text).map((part) => part.text).join("");
}

/** The thread's row for a card: what it asks while open, what it asked once answered. */
export function questionRowWords(question: Question): string {
  const row = askedRow(question);
  if (row) return plainCode(`Asked: ${row.asked}${row.more > 0 ? ` +${row.more} more` : ""} → ${row.answer}`);
  const open = questionOpen(question);
  switch (question.kind.type) {
    case "merge":
      return open ? `Asking whether to merge into ${question.kind.base}` : `Asked whether to merge into ${question.kind.base}`;
    case "uncommittedChanges":
      return open ? "Asking about your uncommitted changes" : "Asked about your uncommitted changes";
    case "orchestrator": {
      if (open) return "Asking questions";
      const count = questionRound(question).length;
      const asked = count === 1 ? "Asked a question" : `Asked ${count} questions`;
      return question.answer === null ? `${asked} · withdrawn` : asked;
    }
  }
}

/**
 * A card line's parts: plain text, and the `code` spans written in backticks (a branch, a
 * file), without the backticks. An unclosed backtick stays as written.
 */
export function codeParts(text: string): { text: string; code: boolean }[] {
  return text
    .split(/`([^`\n]+)`/)
    .map((part, index) => ({ text: part, code: index % 2 === 1 }))
    .filter((part) => part.text !== "");
}
