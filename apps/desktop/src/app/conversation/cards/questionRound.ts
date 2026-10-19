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

/** An answer as the thread shows it: the recommended option says so. */
export function answerWords(item: QuestionItem, answer: string): string {
  if (!answer.trim()) return "No answer provided";
  const recommended = item.recommended === null ? undefined : item.options[item.recommended];
  return recommended?.label === answer ? `${answer} (Recommended)` : answer;
}

/** Text with its `code` spans' backticks left out, for where it can't show code (a tooltip, a label). */
export function plainCode(text: string): string {
  return codeParts(text).map((part) => part.text).join("");
}

/**
 * The thread's row for a card: what it asks while open ("Asking questions"), how many it asked
 * once answered ("Asked 3 questions"), each question and its answer under the row.
 */
export function questionRowWords(question: Question): string {
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
