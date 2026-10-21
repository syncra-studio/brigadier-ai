import type { ReactNode } from "react";

import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { WorkerGlyph } from "@/components/conversation";
import { WORKERS_LABEL } from "@/app/side-panel/workersLabel";
import { TooltipIconButton } from "@/components/controls";
import { ArrowLeft } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import type { Task } from "@/ipc/generated";

/**
 * A worker's header: back, its glyph and name, its model (a click shows its details), Stop and
 * the panel's own actions.
 */
export function WorkerHeader({
  task,
  name,
  model,
  details,
  onBack,
  onToggleDetails,
  headerActions,
}: {
  task: Task;
  name: string | null;
  model: string;
  details: boolean;
  onBack: () => void;
  onToggleDetails: () => void;
  headerActions?: ReactNode;
}) {
  return (
    <header data-slot="worker-header" className="border-border flex h-page-toolbar min-w-0 shrink-0 items-center gap-2 border-b px-2">
      <TooltipIconButton
        tooltip={details ? "Back to worker" : `Back to ${WORKERS_LABEL.toLowerCase()}`}
        size="icon-sm"
        onClick={onBack}
      >
        <ArrowLeft />
      </TooltipIconButton>
      <WorkerGlyph taskId={task.id} className="size-6" />
      <h2 className="min-w-8 truncate text-label font-medium" title={name ?? task.title}>
        {name ?? task.title}
      </h2>
      {/* Its model at the end, as a worker's conversation reads; a click shows its details.
          Where the header is short it gives way first: the title keeps its room. No spacer and
          no padding of its own, which would hold width the title needs once the model is gone. */}
      <Tooltip>
        <TooltipTrigger asChild>
          <button type="button" aria-label={`Worker details: ${model}`} aria-pressed={details}
            data-state={details ? "open" : "closed"}
            className="text-muted-foreground hover:text-foreground data-[state=open]:text-foreground ms-auto flex min-w-0 shrink-1000 rounded-sm text-label transition-colors"
            onClick={onToggleDetails}>
            <span className="truncate">{model}</span>
          </button>
        </TooltipTrigger>
        <TooltipContent>Worker details · {model}</TooltipContent>
      </Tooltip>
      <StopWorker taskId={task.id} name={name ?? task.title} />
      {headerActions}
    </header>
  );
}
