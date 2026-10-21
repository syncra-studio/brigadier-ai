import type { FC } from "react";

import { WorkerGlyph } from "@/app/conversation/WorkerChip";

/** The glyphs of a sentence's workers, up front: one each, the first four. */
export const Glyphs: FC<{ taskIds: readonly string[] }> = ({ taskIds }) => (
  <span aria-hidden className="inline-flex shrink-0 items-center gap-1.5">
    {taskIds.slice(0, 4).map((id) => (
      <WorkerGlyph key={id} taskId={id} className="size-4" />
    ))}
  </span>
);
