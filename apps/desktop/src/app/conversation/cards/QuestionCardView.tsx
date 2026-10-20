import { memo } from "react";

import { CodeSpans } from "@/app/conversation/cards/codeSpans";
import {
  answerWords,
  questionAnswers,
  questionOpen,
  questionRound,
  plainCode,
  questionRowWords,
} from "@/app/conversation/cards/questionRound";
import { MarkdownBlock } from "@/components/conversation/thread";
import { CHEVRON, OPENS, ROW, ROW_TOGGLE } from "@/components/conversation/elements/activity-row";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout/collapsible";
import { ChevronRightAlt, QuestionMarkCircle } from "@/components/icons";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * A question card in the thread's work: one row. While its card waits in the composer's place,
 * "Asking questions", shimmering; once answered, "Asked 3 questions", which opens to each
 * question and the answer given; withdrawn when the user changed the request that asked.
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
        <span className={cn("truncate", questionOpen(question) && "shimmer")}>{words}</span>
      </div>
    );
  }
  const round = questionRound(question);
  return (
    <Collapsible data-slot="question-row">
      <CollapsibleTrigger title={round.map((item) => plainCode(item.text)).join("\n")} className={cn(ROW, ROW_TOGGLE)}>
        {icon}
        <span className="truncate">{words}</span>
        <ChevronRightAlt aria-hidden className={CHEVRON} />
      </CollapsibleTrigger>
      <CollapsibleContent className={OPENS}>
        <dl className="flex flex-col gap-3 pt-2 pb-1 text-sm">
          {round.map((item, index) => (
            <div key={index} className="flex flex-col gap-1">
              <dt className="text-foreground/60 wrap-anywhere [&_.aui-md>:first-child]:mt-0 [&_.aui-md>:last-child]:mb-0"><MarkdownBlock text={item.text} /></dt>
              <dd className="text-foreground/30 whitespace-pre-wrap wrap-anywhere">
                <CodeSpans text={answerWords(item, answers[index] ?? "")} />
              </dd>
            </div>
          ))}
        </dl>
      </CollapsibleContent>
    </Collapsible>
  );
});
