import type { FC } from "react";

import type { BlockCompaction } from "@/app/conversation/blocks";
import { CompactionRow } from "@/app/conversation/CompactionRow";
import { headerLabel, useElapsed } from "@/app/conversation/requestTime";

/**
 * A compaction the user asked for between turns, after the answer it followed: while it runs
 * a turn of its own ("Working for 5s" over the shimmering row), then only its row, with no
 * header or actions.
 */
export const CompactionBlock: FC<{ compaction: BlockCompaction }> = ({ compaction }) => {
  const live = compaction.state === "running";
  const elapsed = useElapsed(compaction.startedAtMs, compaction.endedAtMs, live);
  return (
    <div data-slot="compaction-block" className="mt-4 flex flex-col gap-2">
      {live && (
        <div className="border-border flex items-center border-b pb-2">
          <span className="text-muted-foreground text-sm">{headerLabel("working", elapsed)}</span>
        </div>
      )}
      <CompactionRow compaction={compaction} />
    </div>
  );
};
