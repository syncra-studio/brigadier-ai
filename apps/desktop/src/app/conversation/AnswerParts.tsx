/**
 * An answer laid out in its optional parts (Q86): the opening as a card, the parts in view, the
 * folds after `---` in one list with Sources last, and the Ask next questions as chips. The
 * shapes and the splitting are `answerLayout.ts`; this holds a view for each shape.
 */
import { ThreadPrimitive, useAuiState } from "@assistant-ui/react";
import { ChevronRightAlt, Warning, XCircle } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentType, FC, ReactNode } from "react";

import {
  type AnswerBlock,
  type AnswerLayout,
  bottomLineCard,
  type CautionProps,
  type ChanceProps,
  type PartKind,
  type PartProps,
  type PickProps,
  type StepsProps,
} from "@/app/conversation/answerLayout";
import type { AnswerSource } from "@/app/conversation/answerSources";
import { CitationContext } from "@/components/assistant-ui/file-links";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const Md: FC<{ text: string }> = ({ text }) => <MarkdownBlock text={text} plainLeads />;

/** Markdown inside a part: no paragraph margins of its own. */
const Inline: FC<{ text: string; className?: string }> = ({ text, className }) => (
  <div className={cn("min-w-0 [&_.aui-md-p]:my-0", className)}>
    <Md text={text} />
  </div>
);

/** Steps to do in order: numbered circles joined by a line, each step's phrase over its detail. */
const Steps: FC<StepsProps> = ({ steps }) => (
  <ol data-slot="answer-steps" className="flex flex-col">
    {steps.map((step, index) => (
      <li key={index} className="relative flex gap-3 pb-4 last:pb-0">
        {index < steps.length - 1 && <span aria-hidden className="bg-border absolute top-7 bottom-1 start-2.75 w-px" />}
        <span
          aria-hidden
          className="bg-foreground/10 text-foreground flex size-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold tabular-nums"
        >
          {index + 1}
        </span>
        <div className="min-w-0 pt-0.5">
          <Inline text={step.title} />
          {step.detail && <Inline text={step.detail} className="text-foreground/70" />}
        </div>
      </li>
    ))}
  </ol>
);

/** A trap to avoid: the answer's one accent, a warning title over its signs, each with a cross. */
const Caution: FC<CautionProps> = ({ title, items }) => (
  <div data-slot="answer-avoid" className="bg-foreground/5 rounded-xl px-4 py-3">
    {title && (
      <div className="mb-1.5 flex items-center gap-2 text-sm font-semibold">
        <Warning aria-hidden className="text-warning size-4 shrink-0" />
        <Inline text={title} className="[&_.aui-md-p]:font-semibold" />
      </div>
    )}
    <ul className="flex flex-col gap-1.5">
      {items.map((item, index) => (
        <li key={index} className="flex gap-2">
          <XCircle aria-hidden className="text-warning/80 mt-0.75 size-4 shrink-0" />
          <Inline text={item} />
        </li>
      ))}
    </ul>
  </div>
);

/** "80 of 100" as 100 dots, 80 lit: odds a reader sees rather than computes. */
const Chance: FC<ChanceProps> = ({ of, caption }) => (
  <figure data-slot="answer-chance" className="flex items-center gap-4">
    <div aria-hidden className="grid shrink-0 grid-cols-10 gap-0.75">
      {Array.from({ length: 100 }, (_, index) => (
        <span key={index} className={cn("size-2 rounded-full", index < of ? "bg-warning/85" : "bg-foreground/15")} />
      ))}
    </div>
    <figcaption className="text-sm">
      <span className="text-foreground text-base font-semibold">{of} out of 100</span>
      {caption && (
        <>
          <br />
          <span className="text-foreground/70">{caption}</span>
        </>
      )}
    </figcaption>
  </figure>
);

/** The comparison: the thread's table, the pick's row tinted and badged "My pick for you". */
const Pick: FC<PickProps> = ({ markdown }) => (
  <div data-slot="answer-pick-table" className="[&_tr:has([data-slot=answer-pick])]:bg-success/[0.06] [&_tr:has([data-slot=answer-pick])_td:first-child]:font-medium">
    <Md text={markdown} />
  </div>
);

/**
 * Each shape's view, the other half of `PART_SHAPES`: the type has one for every shape. A view
 * may be an installed assistant-ui element as well as our own: it takes the shape's props.
 */
const PART_VIEWS: { [K in PartKind]: ComponentType<PartProps<K>> } = {
  caution: Caution,
  chance: Chance,
  pick: Pick,
  steps: Steps,
};

const Block: FC<{ block: AnswerBlock }> = ({ block }) => {
  if (block.kind === null) return <Inline text={block.text} className="[&_.aui-md-h3]:mb-1" />;
  const View = PART_VIEWS[block.kind] as ComponentType<typeof block.props>;
  return <View {...block.props} />;
};

/**
 * One fold of an answer: a full-width row under a hairline, its chevron always shown at the start
 * (a fold nobody sees opens is a fold nobody opens), the whole row its button. Several may be
 * open; none is remembered.
 */
export const AnswerFold: FC<{ title: ReactNode; slot?: string; children: ReactNode }> = ({ title, slot = "answer-fold", children }) => (
  <details data-slot={slot} className="group/fold border-border/60 border-b last:border-b-0">
    <summary className="hover:text-foreground focus-visible:text-foreground text-foreground/75 rounded-control flex cursor-pointer list-none items-center gap-2 py-2.5 text-sm [&::-webkit-details-marker]:hidden">
      <ChevronRightAlt
        aria-hidden
        className="size-chevron ease-standard shrink-0 opacity-70 transition-[rotate] duration-300 group-open/fold:rotate-90 motion-reduce:transition-none"
      />
      {title}
    </summary>
    <div className="ps-6 pb-3">{children}</div>
  </details>
);

/** The sources list, in its fold: "Sources · N". */
export const SourcesFold: FC<{ sources: readonly AnswerSource[] }> = ({ sources }) => (
  <AnswerFold slot="answer-sources" title={`Sources · ${sources.length}`}>
    <div data-slot="answer-source-list" className="text-sm">
      <MarkdownBlock text={sources.map((source) => `${source.n}. [${source.title.replace(/[[\]]/g, "")}](<${source.url}>)`).join("\n")} />
    </div>
  </AnswerFold>
);

/** The questions a reader likely asks next: a click sends one, as if typed. */
const AskNext: FC<{ questions: string[] }> = ({ questions }) => {
  // Only the latest answer offers them: an older answer's next questions have passed.
  const latest = useAuiState((s) => s.message.isLast);
  if (!latest) return null;
  return (
    <div data-slot="answer-ask-next" className="flex flex-wrap gap-2">
      {questions.map((question) => (
        <ThreadPrimitive.Suggestion key={question} prompt={question} send clearComposer={false} asChild>
          <Button variant="outline" size="sm" className="rounded-capsule text-foreground/80 h-auto min-h-(--spacing-control-sm) py-1 text-start font-normal whitespace-normal">
            {question}
          </Button>
        </ThreadPrimitive.Suggestion>
      ))}
    </div>
  );
};

/** A finished answer laid out in its parts. */
export const AnswerParts: FC<{ layout: AnswerLayout }> = ({ layout }) => {
  const cite = (n: number) => layout.sources?.find((source) => source.n === n) ?? null;
  return (
    <CitationContext.Provider value={cite}>
      <div data-slot="answer-layout" className="flex flex-col gap-4">
        {layout.bottomLine !== null &&
          (bottomLineCard(layout) ? (
            <div data-slot="answer-bottom-line" className="bg-foreground/5 rounded-xl px-4 py-3 text-base leading-relaxed font-medium">
              <Inline text={layout.bottomLine} />
            </div>
          ) : (
            <Inline text={layout.bottomLine} />
          ))}
        {layout.blocks.map((block, index) => (
          <Block key={index} block={block} />
        ))}
        {(layout.folds.length > 0 || layout.sources) && (
          <div data-slot="answer-more" className="border-border/60 border-t">
            {layout.folds.map((fold, index) => (
              <AnswerFold key={index} title={fold.title}>
                <div className="flex flex-col gap-3">
                  {fold.blocks.map((block, at) => (
                    <Block key={at} block={block} />
                  ))}
                </div>
              </AnswerFold>
            ))}
            {layout.sources && <SourcesFold sources={layout.sources} />}
          </div>
        )}
        {layout.askNext.length > 0 && <AskNext questions={layout.askNext} />}
      </div>
    </CitationContext.Provider>
  );
};
