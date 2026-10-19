/**
 * A round of an interview waiting above the composer (the phase-flow fixture's real
 * ConversationView): its title over the question, and the recommended option marked. Written
 * into `#question-card-title-result`.
 */
import type { Question } from "@/ipc/generated";
import { useBoard } from "@/state/board";

history.replaceState(null, "", `${location.pathname}?view=done&approvals=0&summary=0&sidebar=0`);
await import("@/fixtures/flow");

const conversationId = "flow-session";
const question: Question = {
  id: "round", conversationId, taskId: null, requestId: null, position: 999,
  kind: { type: "orchestrator" }, text: "", options: [], recommended: null,
  title: "Round 1 · Location",
  items: [
    {
      text: "Rent near the central market is about 9 EUR/m², the centre 15. Where should the shop be?",
      options: [
        { label: "Near the market", description: "Lowest rent, foot traffic from makers" },
        { label: "City centre", description: "More walk-ins, nearly twice the rent" },
      ],
      recommended: 0,
    },
    {
      text: "Which filaments do you sell first?",
      options: [{ label: "PETG and PLA", description: "What most people print" }, { label: "Everything", description: null }],
      recommended: 0,
    },
  ],
  answer: null, answers: [], createdAtMs: Date.now(), answeredAtMs: null,
};

const pause = (ms = 100) => new Promise((resolve) => setTimeout(resolve, ms));
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const card = () => document.querySelector<HTMLElement>('[data-slot="action-card"]');

const seen: Record<string, unknown> = {};
try {
  for (let tries = 0; tries < 80 && !useBoard.getState().board?.loaded; tries += 1) await pause();
  useBoard.setState((s) => ({ board: s.board && { ...s.board, questions: { ...s.board.questions, round: question } } }));
  for (let tries = 0; tries < 80 && !card(); tries += 1) await pause();
  if (!card()) throw new Error("no question card above the composer");
  await pause(500);
  const kind = card()!.querySelector('[data-slot="action-card-kind"]');
  const heading = card()!.querySelector("h2");
  Object.assign(seen, {
    kind: text(kind),
    question: text(heading),
    kindAbove: !!kind && !!heading && kind.getBoundingClientRect().bottom <= heading.getBoundingClientRect().top + 1,
    cardText: text(card()),
  });
} catch (error) {
  seen.error = String(error);
}
const result = document.createElement("pre");
result.id = "question-card-title-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
