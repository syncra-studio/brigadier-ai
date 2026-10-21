import { type FC, useEffect, useState } from "react";

import type { BlockCompaction } from "@/app/conversation/blocks";
import { ROW } from "@/components/conversation";
import { TextShorterConcise } from "@/components/icons";
import { cn } from "@/lib/utils";

function compactionLabel({ automatic, state }: BlockCompaction): string {
  switch (state) {
    case "running":
      return "Compacting context";
    case "done":
      return automatic ? "Context automatically compacted" : "Context compacted";
    case "failed":
      return "Couldn’t compact context";
  }
}

/** After this long a compaction says it may take a while. */
const SLOW_COMPACTION_MS = 10_000;

/**
 * The compaction line: "Compacting context" shimmers while it runs (after 10 s, "· This can take
 * a few minutes"), then stays grey.
 */
export const CompactionRow: FC<{ compaction: BlockCompaction }> = ({ compaction }) => {
  const running = compaction.state === "running";
  const [slow, setSlow] = useState(() => running && Date.now() - compaction.startedAtMs >= SLOW_COMPACTION_MS);
  useEffect(() => {
    if (!running || slow) return;
    const timer = window.setTimeout(() => setSlow(true), compaction.startedAtMs + SLOW_COMPACTION_MS - Date.now());
    return () => window.clearTimeout(timer);
  }, [running, slow, compaction.startedAtMs]);
  return (
    <div data-slot="compaction" data-state={compaction.state} className={ROW}>
      <TextShorterConcise aria-hidden className="size-4 shrink-0" />
      <span className={cn("min-w-0 truncate", running && "shimmer")}>
        {compactionLabel(compaction)}
        {running && slow && " · This can take a few minutes"}
        {compaction.error && ` · ${compaction.error}`}
      </span>
    </div>
  );
};
