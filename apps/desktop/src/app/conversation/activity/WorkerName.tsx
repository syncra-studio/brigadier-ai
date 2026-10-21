import { type FC, useContext } from "react";

import { AgentsPanelContext, useWorkerName, WorkerGlyph } from "@/app/conversation/WorkerChip";

/**
 * A worker's name as a button that opens its thread, with its glyph unless the sentence shows
 * the glyphs up front. The one way a worker is named in the thread, the live line and the strip.
 */
export const WorkerName: FC<{ taskId: string; glyph?: boolean }> = ({ taskId, glyph = true }) => {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(taskId);
  if (name === null) return <span className="shrink-0">a worker</span>;
  return (
    <button
      type="button"
      data-slot="worker-name"
      onClick={(event) => {
        event.stopPropagation();
        setPanel(taskId);
      }}
      title={`Open ${name}`}
      className="text-foreground/90 hover:text-foreground rounded-control inline-flex max-w-full min-w-0 items-center gap-1 align-bottom"
    >
      {glyph && <WorkerGlyph taskId={taskId} className="size-icon-xs shrink-0" />}
      <span className="min-w-0 truncate">{name}</span>
    </button>
  );
};
