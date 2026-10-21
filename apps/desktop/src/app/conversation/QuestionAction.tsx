import type { ReactNode } from "react";
import { create } from "zustand";

import { CodeSpans } from "@/app/conversation/cards/codeSpans";
import { questionRound, uncommittedCard } from "@/app/conversation/cards/questionRound";
import { ChoiceCard } from "@/app/conversation/ChoiceCard";
import { StepCounter } from "@/app/conversation/StepCounter";
import { useAction } from "@/app/conversation/useAction";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import { ActionFileList } from "@/components/conversation";
import type { AnswerInput } from "@/ipc/generated";
import { answerQuestion } from "@/state/actions";
import { useBoard } from "@/state/board";

/** Where the user is on an open question card: what a visit to Settings must not lose. */
type KeptQuestion = {
  page: number;
  answers: readonly (string | null)[];
  /** Text typed on a question but not sent yet. */
  drafts: readonly (string | undefined)[];
  /** The answer lit on each question; -1 is the free-text row, null none. */
  lit: readonly (number | null | undefined)[];
};

const FRESH: KeptQuestion = { page: 0, answers: [], drafts: [], lit: [] };

/** Open question cards' state, by card id, until each is answered. */
const useKept = create<{ byCard: Readonly<Record<string, KeptQuestion>> }>(() => ({
  byCard: {},
}));

function keepQuestion(id: string, change: Partial<KeptQuestion>): void {
  useKept.setState((s) => ({
    byCard: { ...s.byCard, [id]: { ...(s.byCard[id] ?? FRESH), ...change } },
  }));
}

function forgetQuestion(id: string): void {
  useKept.setState((s) => {
    const byCard = { ...s.byCard };
    delete byCard[id];
    return { byCard };
  });
}

// A question answered elsewhere, withdrawn or dismissed takes its kept state with it.
useBoard.subscribe(({ board }, { board: before }) => {
  if (!board || board.conversationId !== before?.conversationId) return;
  const { byCard } = useKept.getState();
  for (const id of Object.keys(byCard)) {
    const question = board.questions[id];
    if (before.questions[id] && (!question || question.answer !== null || question.answeredAtMs !== null)) {
      forgetQuestion(id);
    }
  }
});

/** What an answer to a skipped question says, so the asker carries on. */
const SKIPPED = "Skipped: use your best judgment.";

/**
 * A question card from Brigadier, the thread or a worker: its round, one question at a time with
 * a "‹ 1 of 3 ›" pager, and every answer sent at once after the last. Skip tells the asker to
 * use its judgment.
 */
export function QuestionAction({
  id,
  onDismiss,
  footer,
  stagger,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
  stagger: boolean;
}) {
  const question = useBoard((s) => s.board?.questions[id]);
  const askerId = useBoard((s) =>
    question?.taskId && s.board?.tasks[question.taskId]
      ? question.taskId
      : null,
  );
  const action = useAction();
  // Kept by card id, not in the card: opening Settings unmounts it, and coming back finds the
  // round's answers, the text typed and the answer lit as they were.
  const { page, answers, drafts, lit } = useKept((s) => s.byCard[id] ?? FRESH);
  if (!question) return null;

  const round = questionRound(question);
  const at = Math.min(page, round.length - 1);
  const item = round[at]!;
  const uncommitted = uncommittedCard(question);
  // Back to any question answered so far, or on to the first one still open.
  const reachable = (to: number) =>
    to >= 0 && to < round.length && to <= answers.filter((answer) => answer !== null).length;
  const setPage = (to: number) => keepQuestion(id, { page: to });
  const draft = (text: string | undefined) =>
    keepQuestion(id, { drafts: round.map((_, index) => (index === at ? text : useKept.getState().byCard[id]?.drafts[index])) });
  // A merge card's Skip is its "Not yet": words typed there are a new request instead.
  const skipped = (each: (typeof round)[number]) =>
    question.kind.type === "merge" ? (each.options.at(-1)?.label ?? SKIPPED) : SKIPPED;
  const answer = (text: string, input: AnswerInput) => {
    const next = round.map((_, index) => (index === at ? text : (answers[index] ?? null)));
    keepQuestion(id, { answers: next });
    draft(undefined);
    const open = next.findIndex((given) => given === null);
    if (open >= 0) {
      setPage(open);
      return;
    }
    action.run(() =>
      answerQuestion(question.conversationId, question.id, next.map((given) => given ?? SKIPPED), input).then(() =>
        forgetQuestion(id),
      ),
    );
  };
  const given = answers[at] ?? null;
  const picked = given === null ? -1 : item.options.findIndex((option) => option.label === given);
  const pager =
    round.length > 1 ? (
      <StepCounter
        data-slot="round-pager"
        noun="question"
        at={at}
        count={round.length}
        canBack={reachable(at - 1)}
        canForward={reachable(at + 1)}
        onStep={(step) => setPage(at + step)}
        className="-mt-0.5 h-icon-button-sm"
      />
    ) : undefined;
  return (
    <ChoiceCard
      // A question of the round starts fresh, but keeps an answer given, or text typed, before
      // paging away.
      key={at}
      name="question"
      title={
        uncommitted
          ? uncommitted.title
          : <CodeSpans text={item.text} />
      }
      kind={question.title}
      detail={
        uncommitted ? (
          "Brigadier asks once, before a worker starts from them. They are never committed either way."
        ) : askerId === null ? undefined : (
          <>
            <WorkerChip taskId={askerId} /> waits for this
          </>
        )
      }
      extra={
        uncommitted &&
        uncommitted.files.length > 0 && (
          <ActionFileList
            files={uncommitted.files.map(({ path, threads }) => ({
              path,
              trailing: threads && (
                <span className="text-muted-foreground shrink-0 text-xs">the thread's</span>
              ),
            }))}
          />
        )
      }
      choices={item.options.map((option, index) => ({
        label: option.label,
        description: option.description && <CodeSpans text={option.description} />,
        recommended: item.recommended === index,
      }))}
      kept={lit[at] ?? (picked >= 0 ? picked : null)}
      initialText={drafts[at] ?? (given !== null && picked < 0 && given !== SKIPPED ? given : "")}
      placeholder={
        item.options.length > 0
          ? "No, and tell Brigadier what to do differently"
          : "Type here"
      }
      pager={pager}
      last={answers.filter((other, index) => index !== at && other !== null).length === round.length - 1}
      onPage={(step) => reachable(at + step) && setPage(at + step)}
      onChoose={(index, input) => answer(item.options[index]?.label ?? "", input)}
      onText={(text) => answer(text, "typed")}
      onTyped={draft}
      onLit={(index) =>
        keepQuestion(id, { lit: round.map((_, other) => (other === at ? index : useKept.getState().byCard[id]?.lit[other])) })
      }
      onSkip={(input) => answer(skipped(item), input)}
      onDismiss={onDismiss}
      // × skips what is still open in the round, so the asker hears and carries on (issue #239).
      onClose={{
        label: "Skip",
        run: () =>
          action.run(() =>
            answerQuestion(
              question.conversationId,
              question.id,
              round.map((each, index) => answers[index] ?? skipped(each)),
              "click",
            ).then(() => forgetQuestion(id)),
          ),
      }}
      busy={action.busy}
      error={action.error}
      footer={footer}
      stagger={stagger && at === 0}
    />
  );
}
