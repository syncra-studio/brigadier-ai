import type { ReactNode } from "react";

import type { Described } from "@/app/conversation/activity/ActivityGroup";
import type { GroupItem } from "@/app/conversation/activity/group";
import { InGroup } from "@/app/conversation/activity/groupContext";
import { RepeatsRow } from "@/app/conversation/activity/RepeatsRow";
import { foldRepeats } from "@/app/conversation/activity/words";
import { GROUP_LIST } from "@/components/conversation";

/** Reads, searches and listings that still run show once done inside an opened group: its row names them. */
const exploring = (described: Described) =>
  described.status === "inProgress" && (described.words.kind === "read" || described.words.kind === "code");

/**
 * An opened group's steps: up to 224px of rows 4px apart, then it scrolls, its edges fading while there is more.
 * Its rows keep their height: an opened Shell box scrolls with them, never squeezed.
 */
export function GroupStepList<S>({ items, described, renderStep }: {
  items: readonly GroupItem<S>[];
  described: readonly Described[];
  renderStep: (step: S, key: string) => ReactNode;
}) {
  return (
    <div data-slot={GROUP_LIST} className="max-h-group-list scroll-edge-fade gap-activity-row-gap flex min-w-0 flex-col overflow-y-auto *:shrink-0 pt-1 [--spacing-scroll-fade-bottom:var(--spacing-group-fade)] [--spacing-scroll-fade-top:var(--spacing-group-fade)] [--diff-header-top:var(--spacing-group-fade)]">
      <InGroup.Provider value>
        {foldRepeats(described.map((step) => step.words)).map((run) => {
          const [index = 0] = run;
          const item = items[index];
          if (!item) return null;
          if (run.length > 1) {
            return (
              <RepeatsRow key={`repeats:${item.key}`} described={run.flatMap((at) => described[at] ?? [])}>
                {run.map((at) => items[at] && renderStep(items[at].step, items[at].key))}
              </RepeatsRow>
            );
          }
          return described[index] && exploring(described[index]) ? null : renderStep(item.step, item.key);
        })}
      </InGroup.Provider>
    </div>
  );
}
