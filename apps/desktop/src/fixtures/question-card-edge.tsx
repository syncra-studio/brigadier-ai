/**
 * A plain question waiting above the composer in a finished session (the phase-flow fixture's
 * real ConversationView), in a 1280×820 window: where the card's bottom edge sits against the
 * composer under it, and whether that edge is drawn, written into `#question-card-edge-result`.
 */
import { fixtureInput } from "@/fixtures/input";
import type { Question } from "@/ipc/generated";
import { useBoard } from "@/state/board";

history.replaceState(null, "", `${location.pathname}?view=done&approvals=0&summary=0&sidebar=0`);
await import("@/fixtures/flow");

const conversationId = "flow-session";
const question: Question = {
  id: "edge", conversationId, taskId: null, requestId: null, position: 999,
  kind: { type: "orchestrator" }, text: "", options: [], recommended: null,
  items: [{
    text: "Which discount kinds should the checkout support?",
    options: [
      { label: "Percent off", description: "A share of the order total." },
      { label: "Fixed amount", description: "A set sum off the order." },
      { label: "Both", description: null },
    ],
    recommended: 0,
  }],
  answer: null, answers: [], createdAtMs: Date.now(), answeredAtMs: null,
};

const pause = (ms = 100) => new Promise((resolve) => setTimeout(resolve, ms));
/** The card's place on the rail: what draws its edges. */
const card = () => document.querySelector<HTMLElement>('[data-slot="composer-rail-item"]:has([data-slot="action-card"])');

const seen: Record<string, unknown> = {};
try {
  for (let tries = 0; tries < 80 && !useBoard.getState().board?.loaded; tries += 1) await pause();
  useBoard.setState((s) => ({ board: s.board && { ...s.board, questions: { ...s.board.questions, edge: question } } }));
  await fixtureInput({ type: "viewport", width: 1280, height: 820 });
  for (let tries = 0; tries < 80 && !card(); tries += 1) await pause();
  if (!card()) throw new Error("no question card above the composer");
  // The rail opens to its height.
  await pause(800);
  const shell = document.querySelector<HTMLElement>('[data-slot="aui_composer-shell"]')!;
  const box = card()!.getBoundingClientRect();
  const style = getComputedStyle(card()!);
  Object.assign(seen, {
    window: [innerWidth, innerHeight],
    cardBottom: Math.round(box.bottom),
    composerTop: Math.round(shell.getBoundingClientRect().top),
    border: Number.parseFloat(style.borderBottomWidth),
    radius: Number.parseFloat(style.borderBottomLeftRadius),
  });
} catch (error) {
  seen.error = String(error);
}
const result = document.createElement("pre");
result.id = "question-card-edge-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
