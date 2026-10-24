import { TooltipIconButton } from "@/components/controls";
import { X } from "@/components/icons";
import type { SentToWorker } from "@/ipc/generated";

/** A queued message the session sent on to one of its workers: where it went, with ×. */
export function SentToWorkerNotice({
  notice,
  onDismiss,
}: {
  notice: SentToWorker;
  onDismiss: () => void;
}) {
  return (
    <li
      data-slot="message-queue-sent"
      className="text-muted-foreground min-h-row flex items-center gap-2 ps-3 pe-1 text-sm"
    >
      <span className="min-w-0 flex-1 truncate" title={notice.text}>
        Sent to worker #{notice.workerNumber} “{notice.workerTitle}”: {notice.text}
      </span>
      <TooltipIconButton
        tooltip="Dismiss"
        side="top"
        className="text-muted-foreground"
        onClick={onDismiss}
      >
        <X />
      </TooltipIconButton>
    </li>
  );
}
