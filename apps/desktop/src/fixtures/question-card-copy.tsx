/**
 * A question waiting above the composer (the phase-flow fixture's real ConversationView) whose
 * answer names a long command (#663): the question's words are selected by a real drag across
 * them, and the answer's copy button is clicked, as the user's pointer does. What was selected,
 * what went to the clipboard, and whether an answer was sent go into `#question-card-copy-result`.
 */
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Question, Request } from "@/ipc/generated";
import { useBoard } from "@/state/board";

history.replaceState(null, "", `${location.pathname}?view=done&approvals=0&summary=0&sidebar=0`);
await import("@/fixtures/flow");

const command = "Get-ChildItem -Path $env:TEMP -Filter 'brigadier-*' -Recurse | Remove-Item -Recurse -Force";
const conversationId = "flow-session";
const question: Question = {
  id: "cleanup", conversationId, taskId: null, requestId: null, position: 999,
  kind: { type: "orchestrator" }, text: "", options: [], recommended: null, title: null,
  items: [
    {
      text: "Clean up the leftover `brigadier-*` folders in TEMP now?",
      options: [
        { label: "Run the cleanup", description: `Runs \`${command}\`` },
        { label: "Leave them", description: null },
      ],
      recommended: 0,
    },
  ],
  answer: null, answers: [], createdAtMs: Date.now(), answeredAtMs: null,
};

// What the copy button writes, kept here: a headless page's clipboard may refuse it.
const copied: string[] = [];
Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: { writeText: (text: string) => (copied.push(text), Promise.resolve()), write: () => Promise.resolve() },
});

const pause = (ms = 100) => new Promise((resolve) => setTimeout(resolve, ms));
const card = () => document.querySelector<HTMLElement>('[data-slot="action-card"]');
const calls = () => (window as unknown as { flow: { calls: Request[] } }).flow.calls;

const seen: Record<string, unknown> = {};
try {
  for (let tries = 0; tries < 80 && !useBoard.getState().board?.loaded; tries += 1) await pause();
  useBoard.setState((s) => ({ board: s.board && { ...s.board, questions: { ...s.board.questions, cleanup: question } } }));
  for (let tries = 0; tries < 80 && !card(); tries += 1) await pause();
  if (!card()) throw new Error("no question card above the composer");
  await pause(500);
  // A drag from the question's first word to its last.
  const heading = card()!.querySelector("h2")!;
  const box = heading.getBoundingClientRect();
  const y = box.top + Math.min(box.height / 2, 8);
  await fixtureInput({ type: "pointer", x: box.left + 1, y });
  await fixtureInput({ type: "pointerDown", x: box.left + 1, y });
  for (const step of [0.25, 0.5, 0.75, 1]) await fixtureInput({ type: "pointer", x: box.left + 1 + (box.width - 2) * step, y: box.bottom - 2 });
  await fixtureInput({ type: "pointerUp", x: box.right - 1, y: box.bottom - 2 });
  seen.selected = document.getSelection()?.toString().trim() ?? "";
  // The command's copy button, beside its answer.
  const copy = card()!.querySelector<HTMLElement>('[data-slot="action-option-copy"]');
  seen.copyButtons = card()!.querySelectorAll('[data-slot="action-option-copy"]').length;
  seen.copyInsideAnswer = !!copy?.closest('[data-slot="action-option"]');
  if (copy) await fixtureClick(copy);
  // Longer than a picked answer takes to go (ChoiceCard's COMMIT_MS).
  await pause(600);
  seen.copied = copied;
  seen.answered = calls().some((call) => call.method === "answerQuestion");
  seen.stillAsks = !!card() && useBoard.getState().board?.questions.cleanup?.answer === null;
} catch (error) {
  seen.error = String(error);
}
const result = document.createElement("pre");
result.id = "question-card-copy-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
