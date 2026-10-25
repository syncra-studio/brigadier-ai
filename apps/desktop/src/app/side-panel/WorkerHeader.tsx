import type { ReactNode } from "react";

import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { WorkerGlyph } from "@/components/conversation";
import { WORKERS_LABEL } from "@/app/side-panel/workersLabel";
import { TooltipIconButton } from "@/components/controls";
import { ArrowLeft, Chat } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import type { WorkerRow } from "@/ipc/generated";

/**
 * A worker's header, from its row: back, its glyph and name, its model and state (a click shows
 * its details, when it has them), Message (the composer's To:), Stop and the panel's own actions.
 */
export function WorkerHeader({
  row,
  name,
  model,
  state,
  details,
  onBack,
  onToggleDetails,
  onMessage,
  headerActions,
}: {
  row: WorkerRow;
  name: string | null;
  model: string;
  /** Its state in a word or two (`workerStateOf`). */
  state: string;
  details: boolean;
  onBack: () => void;
  onToggleDetails?: (() => void) | undefined;
  /** Sets the session composer's To: to it, while it can take a message from the user. */
  onMessage?: (() => void) | undefined;
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
      <WorkerGlyph taskId={row.id} className="size-6" />
      <h2 className="min-w-8 truncate text-label font-medium" title={name ?? row.title}>
        {name ?? row.title}
      </h2>
      {/* Its model at the end, as a worker's conversation reads; a click shows its details.
          Where the header is short it gives way first: the title keeps its room. No spacer and
          no padding of its own, which would hold width the title needs once the model is gone. */}
      <Tooltip>
        <TooltipTrigger asChild>
          <button type="button" aria-label={`Worker details: ${model} · ${state}`} aria-pressed={details}
            data-state={details ? "open" : "closed"}
            data-slot="worker-header-model"
            disabled={!onToggleDetails}
            className="text-muted-foreground hover:text-foreground data-[state=open]:text-foreground ms-auto flex min-w-0 shrink-1000 rounded-sm text-label transition-colors"
            onClick={onToggleDetails}>
            <span className="truncate">{model} · {state}</span>
          </button>
        </TooltipTrigger>
        <TooltipContent>Worker details · {model} · {state}</TooltipContent>
      </Tooltip>
      {onMessage && (
        <TooltipIconButton tooltip="Message this worker" size="icon-sm" data-slot="worker-message" onClick={onMessage}>
          <Chat />
        </TooltipIconButton>
      )}
      <StopWorker row={row} name={name ?? row.title} />
      {headerActions}
    </header>
  );
}
