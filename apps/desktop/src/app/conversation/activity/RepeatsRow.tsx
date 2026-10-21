import type { ReactNode } from "react";

import type { Described } from "@/app/conversation/activity/ActivityGroup";
import { InFold } from "@/app/conversation/activity/groupContext";
import { timesWord } from "@/app/conversation/activity/words";
import { ROW_DETAIL, StepIcon, ThreadActivity } from "@/components/conversation";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * Back-to-back calls of one tool Brigadier has no words for, as one row of an opened group:
 * "Used search issues 6 times in 12s". It opens to each call, indented, saying what it was given (#101).
 */
export function RepeatsRow({ described, children }: { described: readonly Described[]; children: ReactNode }) {
  const [first] = described;
  if (!first) return null;
  const running = described.some((step) => step.status === "inProgress");
  const took = described.map((step) => (step.startedAtMs !== undefined && step.endedAtMs !== undefined ? step.endedAtMs - step.startedAtMs : undefined));
  const ms = took.every((each) => each !== undefined) ? took.reduce((sum, each) => sum + each, 0) : 0;
  return (
    <ThreadActivity
      data-slot="repeats"
      detail={<InFold.Provider value>{children}</InFold.Provider>}
      detailClassName={cn(ROW_DETAIL, "gap-activity-row-gap max-h-none overflow-visible")}
    >
      <StepIcon words={first.words} />
      <span className={cn("min-w-0 truncate", running && "shimmer")}>{`${running ? first.words.doing : first.words.done} ${timesWord(described.length)}`}</span>
      {!running && ms >= 1000 && <span className="shrink-0 tabular-nums">{`in ${formatDuration(ms)}`}</span>}
    </ThreadActivity>
  );
}
