import { ChevronRight, Globe } from "@openai/apps-sdk-ui/components/Icon";
import type { ReactNode } from "react";

import type { GroupItem, Thought as ThoughtItem } from "@/app/conversation/activity/group";
import { stepLabel, type StepWords, summarize } from "@/app/conversation/activity/words";
import { ThinkingRow } from "@/app/conversation/ThinkingRow";
import { CHEVRON, OPENS, ROW, ROW_DETAIL, ROW_TOGGLE, WORK_ICONS } from "@/components/assistant-ui/elements/activity-row";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { ItemStatus } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A step's words and how it stands, as a group reads them; a command's exit code when it says. */
export type Described = { words: StepWords; status: ItemStatus; exit?: number | null };

function StepIcon({ words }: { words: StepWords }) {
  const Icon = words.web ? Globe : WORK_ICONS[words.kind];
  return <Icon aria-hidden className="size-4 shrink-0" />;
}

function Thought({ thought }: { thought: ThoughtItem }) {
  return <ThinkingRow text={thought.text} ms={thought.ms} live={false} />;
}

/**
 * One step as a row: "Read RequestBlock.tsx", "Ran pnpm test in 41s"; the verb shimmers while it
 * runs. It opens to `detail` when there is one.
 */
export function StepRow({ words, status, exit = null, detail, suffix, slot }: Described & {
  detail?: ReactNode;
  /** After the words: "in 41s". */
  suffix?: ReactNode;
  slot?: string;
}) {
  return (
    // Its Shell box or diff spans the column, flush under the row.
    <ThreadActivity data-slot={slot} data-kind={words.kind} detail={detail} detailClassName={cn(ROW_DETAIL, "ps-0")}>
      <StepIcon words={words} />
      <span className={cn("min-w-0 truncate", status === "inProgress" && "shimmer")}>{stepLabel(words, status, exit)}</span>
      {suffix && <span className="shrink-0 tabular-nums">{suffix}</span>}
    </ThreadActivity>
  );
}

/**
 * A group of work as one row (THREAD-UX-PLAN.md §3.2). Closed, it sums its steps up ("Read
 * files, ran commands"); while it is the turn's open group, it says the current step
 * ("Reading blocks.ts"). Opened, it lists each step and the thinking between them. A group of one
 * step with no thinking is that step's own row.
 */
export function ActivityGroup<S>({ items, live, describe, renderStep }: {
  items: readonly GroupItem<S>[];
  /** It is the live turn's last group: it says what happens now. */
  live: boolean;
  describe: (step: S) => Described;
  renderStep: (step: S, key: string) => ReactNode;
}) {
  const steps = items.flatMap((item) => (item.type === "step" ? [item] : []));
  const [only] = steps;
  if (only && steps.length === 1 && items.length === 1) return <>{renderStep(only.step, only.key)}</>;
  // A turn with thinking and no work at all: its thoughts are its rows.
  if (!only) return <>{items.map((item) => item.type === "thought" && <Thought key={`thought:${item.thought.key}`} thought={item.thought} />)}</>;
  const described = steps.map((item) => describe(item.step));
  const current = described.at(-1);
  const first = described[0];
  const head = live || described.length === 1 ? current : first;
  const label = !current
    ? ""
    : live || described.length === 1
      ? stepLabel(current.words, current.status, current.exit)
      : summarize(described.map((step) => step.words));
  return (
    <Collapsible data-slot="work-group">
      <CollapsibleTrigger className={cn(ROW, ROW_TOGGLE)}>
        {head && <StepIcon words={head.words} />}
        {/* A live group's step changes in place: its words cross-fade, with no jump. */}
        <span
          key={live ? label : undefined}
          className={cn("min-w-0 truncate", live && "animate-in fade-in duration-160 motion-reduce:animate-none", live && current?.status === "inProgress" && "shimmer")}
        >
          {label}
        </span>
        <ChevronRight aria-hidden className={CHEVRON} />
      </CollapsibleTrigger>
      <CollapsibleContent className={OPENS}>
        {/* Up to 224px of rows 4px apart, then it scrolls, its edges fading while there is more. */}
        <div className="max-h-group-list scroll-edge-fade gap-activity-row-gap flex min-w-0 flex-col overflow-y-auto pt-1 [--spacing-scroll-fade-bottom:var(--spacing-group-fade)] [--spacing-scroll-fade-top:var(--spacing-group-fade)]">
          {items.map((item) =>
            item.type === "step" ? (
              renderStep(item.step, item.key)
            ) : (
              <Thought key={`thought:${item.thought.key}`} thought={item.thought} />
            ),
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
