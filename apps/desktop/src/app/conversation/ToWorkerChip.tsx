import { composerPill, WorkerGlyph } from "@/components/conversation";
import { type FC, useContext } from "react";

import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { useWorkerName } from "@/app/conversation/WorkerChip";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { cn } from "@/lib/utils";
import { setDirectTo, useDirectSending, useDirectTo } from "@/state/directTo";

/**
 * The composer's To: chip, as the New session chip: while it shows, the next message goes to
 * that worker alone (picked from the `@` menu's "Message directly"); a click takes it back.
 */
export const ToWorkerChip: FC = () => {
  const target = useContext(ComposerTargetContext);
  const id = target?.conversation?.id ?? null;
  const taskId = useDirectTo(id);
  const name = useWorkerName(taskId ?? "");
  const sending = useDirectSending(id);
  if (!id || !taskId) return null;
  return (
    <>
      <span aria-hidden className="bg-foreground/15 mx-1 h-icon-md w-px" />
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-pressed
            data-slot="to-worker-chip"
            className={cn(composerPill, "text-muted-foreground max-w-48")}
            disabled={sending}
            onClick={() => setDirectTo(id, null)}
          >
            <WorkerGlyph taskId={taskId} />
            <span className="truncate">To {name ?? "worker"}</span>
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom">Send to this session instead</TooltipContent>
      </Tooltip>
    </>
  );
};
