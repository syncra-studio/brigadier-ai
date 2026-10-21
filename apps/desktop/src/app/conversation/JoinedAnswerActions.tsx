import type { FC } from "react";

import { ReadAloud } from "@/app/conversation/ReadAloud";
import type { BlockMeta } from "@/app/conversation/RequestBlock";
import { useViewConversation } from "@/app/conversation/viewContext";
import { TooltipIconButton } from "@/components/controls";
import { RateItem, RateMenu, useAuiState } from "@/components/conversation";
import { Check, Copy } from "@/components/icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { Rating } from "@/ipc/generated";
import { formatSentAt } from "@/lib/format";
import { rateMessage } from "@/state/actions";
import { useBoard } from "@/state/board";
import { toast } from "@/state/toasts";

/**
 * The actions under an answer a follow-up joined, inside its turn: the same Copy, Read aloud,
 * rating and time as under the turn's last answer, for that reply alone.
 */
export const JoinedAnswerActions: FC<{ index: number; text: BlockMeta["texts"][number] }> = ({ index, text }) => {
  const conversation = useViewConversation();
  const answer = useAuiState((s) => {
    const part = s.message.parts[index];
    return part?.type === "text" ? part.text : "";
  });
  const rated = useBoard((s) => s.board?.ratings[text.messageId] ?? null);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  const rate = (rating: Rating | null) => {
    if (!conversation) return;
    rateMessage(conversation.id, text.messageId, rating).catch((error: unknown) =>
      toast(error instanceof Error ? error.message : String(error), { tone: "error" }),
    );
  };
  return (
    <div
      data-slot="answer-actions"
      className="text-foreground/50 gap-answer-actions-between -ms-1 mt-answer-actions-gap flex items-center [&_.aui-button-icon]:size-answer-action"
    >
      <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy response"} onClick={() => copyToClipboard(answer)}>
        {isCopied ? (
          <Check className="animate-in zoom-in-50 fade-in duration-200 ease-out" />
        ) : (
          <Copy className="animate-in zoom-in-75 fade-in duration-150" />
        )}
      </TooltipIconButton>
      <ReadAloud text={answer} />
      <RateMenu rated={rated} onClear={() => rate(null)}>
        <RateItem rating="good" onSelect={() => rate("good")} />
        <RateItem rating="bad" onSelect={() => rate("bad")} />
      </RateMenu>
      {text.atMs !== undefined && (
        <span
          data-slot="answer-time"
          className="ps-1 text-xs tabular-nums opacity-0 group-hover/joined:opacity-100 group-focus-within/joined:opacity-100"
        >
          {formatSentAt(text.atMs, now)}
        </span>
      )}
    </div>
  );
};
