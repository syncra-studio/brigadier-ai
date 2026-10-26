import { type FC, useEffect, useState } from "react";

import type { BlockCompaction } from "@/app/conversation/blocks";
import { TextShorterConcise } from "@/components/icons";
import { LabeledDivider } from "@/components/shared";
import { cn } from "@/lib/utils";

function compactionLabel({ state }: BlockCompaction): string {
  switch (state) {
    case "running":
      return "Refreshing context";
    case "done":
      return "Context refreshed · everything important kept";
    case "failed":
      return "Couldn’t compact context";
  }
}

/** After this long a compaction says it may take a while. */
const SLOW_COMPACTION_MS = 10_000;

/**
 * A compaction, done under the hood: one divider across the thread where it happened, never a
 * turn or a row of one. "Refreshing context" shimmers while it runs (after 10 s, "· This can
 * take a few minutes"), then it says the context was refreshed.
 */
export const CompactionDivider: FC<{ compaction: BlockCompaction }> = ({ compaction }) => {
  const running = compaction.state === "running";
  const [slow, setSlow] = useState(() => running && Date.now() - compaction.startedAtMs >= SLOW_COMPACTION_MS);
  useEffect(() => {
    if (!running || slow) return;
    const timer = window.setTimeout(() => setSlow(true), compaction.startedAtMs + SLOW_COMPACTION_MS - Date.now());
    return () => window.clearTimeout(timer);
  }, [running, slow, compaction.startedAtMs]);
  return (
    <LabeledDivider data-slot="compaction" data-state={compaction.state} role="status">
      <TextShorterConcise aria-hidden className="size-icon-sm shrink-0" />
      <span className={cn("min-w-0 text-center", running && "shimmer")}>
        {compactionLabel(compaction)}
        {running && slow && " · This can take a few minutes"}
        {compaction.error && ` · ${compaction.error}`}
      </span>
    </LabeledDivider>
  );
};
