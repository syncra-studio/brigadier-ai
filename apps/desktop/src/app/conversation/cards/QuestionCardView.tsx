import { ChevronRight, QuestionMarkCircle } from "@openai/apps-sdk-ui/components/Icon";
import { memo } from "react";

import {
  answerWords,
  questionAnswers,
  questionRound,
  questionRowWords,
} from "@/app/conversation/cards/questionRound";
import { CHEVRON, OPENS, ROW } from "@/components/assistant-ui/elements/activity-row";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * A question card in the thread's work: one row. While its card waits in the composer's place,
 * "Asking questions"; once answered, "Asked 3 questions", which opens to each question and the
 * answer given; withdrawn when the user changed the request that asked.
 */
export const QuestionCardView = memo(function QuestionCardView({ cardId }: { cardId: string }) {
  const question = useBoard((s) => s.board?.questions[cardId]);
  if (!question) return null;
  const words = questionRowWords(question);
  const answers = questionAnswers(question);
  const icon = <QuestionMarkCircle aria-hidden className="size-4 shrink-0" />;
  if (answers.length === 0) {
    return (
      <div data-slot="question-row" className={ROW}>
        {icon}
        <span className="truncate">{words}</span>
      </div>
    );
  }
  const round = questionRound(question);
  return (
    <Collapsible data-slot="question-row">
      <CollapsibleTrigger className={cn(ROW, "group hover:text-foreground rounded-control w-fit text-start")}>
        {icon}
        <span className="truncate">{words}</span>
        <ChevronRight aria-hidden className={CHEVRON} />
      </CollapsibleTrigger>
      <CollapsibleContent className={OPENS}>
        <dl className="flex flex-col gap-3 pt-2 pb-1 text-sm">
          {round.map((item, index) => (
            <div key={index} className="flex flex-col gap-1">
              <dt className="text-foreground/60 whitespace-pre-wrap wrap-anywhere">{item.text}</dt>
              <dd className="text-foreground/30 whitespace-pre-wrap wrap-anywhere">
                {answerWords(item, answers[index] ?? "")}
              </dd>
            </div>
          ))}
        </dl>
      </CollapsibleContent>
    </Collapsible>
  );
});
