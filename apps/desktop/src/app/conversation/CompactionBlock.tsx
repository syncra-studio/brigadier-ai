import type { FC } from "react";

import type { BlockCompaction } from "@/app/conversation/blocks";
import { CompactionDivider } from "@/app/conversation/CompactionRow";

/**
 * A compaction between turns, after the answer it followed: only its divider, no turn of its own
 * (no header, no time, no actions), while it runs too.
 */
export const CompactionBlock: FC<{ compaction: BlockCompaction }> = ({ compaction }) => (
  <div data-slot="compaction-block" className="mt-4 flex flex-col">
    <CompactionDivider compaction={compaction} />
  </div>
);
