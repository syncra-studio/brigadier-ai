import type { FC } from "react";

import { HeldNote } from "@/app/conversation/HeldNote";
import { UserMarkdownText } from "@/app/conversation/Mentions";
import { TooltipIconButton } from "@/components/controls";
import { Check, Copy } from "@/components/icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { AttachmentRef } from "@/ipc/generated";
import { formatSentAt } from "@/lib/format";
import { useBoard } from "@/state/board";

/** A follow-up the user sent while the block worked: their bubble, inside the block. */
export const SteerBubble: FC<{ text: string; atMs: number; attachments: readonly AttachmentRef[]; requestId?: string | undefined }> = ({
  text,
  atMs,
  attachments,
  requestId,
}) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  // Sent while the thread is inside a command: it reads it once that ends.
  const held = useBoard((s) => (requestId ? s.board?.requests[requestId]?.heldBy : undefined));
  return (
    <div
      data-slot="request-steer"
      className="group/steer flex max-w-7/10 min-w-0 flex-col items-end gap-y-1 self-end"
    >
      <div className="bg-muted text-foreground rounded-bubble max-w-full min-w-0 px-4 py-2.5 wrap-anywhere [&_.aui-md-inline-code]:bg-foreground/10">
        <UserMarkdownText text={text} attachments={attachments} />
      </div>
      {held && <HeldNote held={held} />}
      <div className="text-muted-foreground flex items-center gap-1 opacity-0 transition-opacity group-hover/steer:opacity-100 group-focus-within/steer:opacity-100">
        <span className="pe-1 text-xs tabular-nums">{formatSentAt(atMs, now)}</span>
        <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy"} onClick={() => copyToClipboard(text)}>
          {isCopied ? <Check /> : <Copy />}
        </TooltipIconButton>
      </div>
    </div>
  );
};
