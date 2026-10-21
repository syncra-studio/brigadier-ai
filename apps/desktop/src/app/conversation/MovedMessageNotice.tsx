import { Button, TooltipIconButton } from "@/components/controls";
import { X } from "@/components/icons";
import type { MovedMessage } from "@/ipc/generated";

/** A queued message sent on to another conversation: where it went, with Undo and ×. */
export function MovedMessageNotice({
  notice,
  busy,
  onUndo,
  onDismiss,
}: {
  notice: MovedMessage;
  busy: boolean;
  onUndo: () => void;
  onDismiss: () => void;
}) {
  return (
    <li
      data-slot="message-queue-moved"
      className="text-muted-foreground min-h-row flex items-center gap-2 ps-3 pe-1 text-sm"
    >
      <span className="min-w-0 flex-1 truncate" title={notice.text}>
        Sent to “{notice.toTitle}”: {notice.text}
      </span>
      <Button
        size="xs"
        variant="ghost"
        disabled={busy}
        onClick={onUndo}
      >
        Undo
      </Button>
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
