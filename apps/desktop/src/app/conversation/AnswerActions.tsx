import type { FC, MouseEvent } from "react";

import { ForkMenu } from "@/app/conversation/ForkMenu";
import { ReadAloud } from "@/app/conversation/ReadAloud";
import { useViewConversation } from "@/app/conversation/viewContext";
import { TooltipIconButton } from "@/components/controls";
import { ActionBarPrimitive, BranchPicker, RateItem, RateMenu, useAuiState } from "@/components/conversation";
import { Check, Copy, Regenerate } from "@/components/icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import { formatSentAt } from "@/lib/format";
import { cn } from "@/lib/utils";
import { rateMessage } from "@/state/actions";
import { toast } from "@/state/toasts";

/**
 * Under the answer: copy it, read it aloud, rate it and, in a Chat, ask for another answer and move between
 * answers (a session's answers have neither). The latest answer always shows them and when it
 * came, an older one on hover or focus. Copy and Read aloud take the text `shown` (the answer and
 * its update lines, by part), or else the last part; Copy also puts the answer as shown (HTML)
 * on the clipboard, for apps that paste rich text.
 */
export const AnswerActions: FC<{
  session: boolean;
  rework: boolean;
  atMs: number | null;
  shown: number[] | null;
  forkId: string | null;
  answerId: string | null;
}> = ({ session, rework, atMs, shown, forkId, answerId }) => {
  const conversation = useViewConversation();
  // One string, so the selector's result compares by value: its parts split at a NUL.
  const texts = useAuiState((s) => {
    const parts = s.message.parts;
    return (shown ?? [parts.length - 1])
      .map((index) => parts[index])
      .map((part) => (part?.type === "text" ? part.text : ""))
      .filter((text) => text !== "")
      .join("\0");
  }).split("\0");
  const answer = texts.join("\n\n");
  const rated = useAuiState((s) => {
    const type = s.message.metadata.submittedFeedback?.type;
    return type === "positive" ? "good" : type === "negative" ? "bad" : null;
  });
  const latest = useAuiState((s) => s.message.isLast);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  const copy = (event: MouseEvent<HTMLElement>) => {
    const turn = event.currentTarget.closest("[data-role=assistant]");
    copyToClipboard(answer, turn ? answerHtml(turn) : undefined);
  };
  return (
    <ActionBarPrimitive.Root
      data-slot="answer-actions"
      autohide="never"
      className={cn(
        // 26px buttons 2px apart, their 16px icons at half white.
        "text-foreground/50 gap-answer-actions-between -ms-1 flex items-center [&_.aui-button-icon]:size-answer-action",
      )}
    >
      <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy response"} onClick={copy}>
        {isCopied ? (
          <Check className="animate-in zoom-in-50 fade-in duration-200 ease-out" />
        ) : (
          <Copy className="animate-in zoom-in-75 fade-in duration-150" />
        )}
      </TooltipIconButton>
      <ReadAloud text={answer} />
      <RateMenu rated={rated} onClear={() => {
        if (conversation && answerId) {
          rateMessage(conversation.id, answerId, null).catch((error: unknown) =>
            toast(error instanceof Error ? error.message : String(error), { tone: "error" }),
          );
        }
      }}>
        <ActionBarPrimitive.FeedbackPositive asChild>
          <RateItem rating="good" />
        </ActionBarPrimitive.FeedbackPositive>
        <ActionBarPrimitive.FeedbackNegative asChild>
          <RateItem rating="bad" />
        </ActionBarPrimitive.FeedbackNegative>
      </RateMenu>
      {!session && rework && (
        <ActionBarPrimitive.Reload asChild>
          <TooltipIconButton tooltip="Try again">
            <Regenerate />
          </TooltipIconButton>
        </ActionBarPrimitive.Reload>
      )}
      {!session && <BranchPicker />}
      {conversation && forkId && (
        <ForkMenu conversationId={conversation.id} kind={conversation.kind} messageId={forkId} />
      )}
      {/* When it came: always under the latest answer, on hover under an older one. */}
      {atMs !== null && (
        <span
          data-slot="answer-time"
          className={cn("ps-1 text-xs tabular-nums", !latest && "opacity-0 group-hover/answer:opacity-100 group-focus-within/answer:opacity-100")}
        >
          {formatSentAt(atMs, now)}
        </span>
      )}
    </ActionBarPrimitive.Root>
  );
};

/**
 * What a rich-text paste of the answer gets: its rendered text, without the thread's controls and
 * styling. Only the answer and its updates, as in the plain text: never the work's own replies,
 * shown or not.
 */
export function answerHtml(turn: Element): string | undefined {
  const parts = [...turn.querySelectorAll("[data-answer], [data-slot=answer-update]")];
  if (parts.length === 0) return undefined;
  return parts
    .map((part) => {
      const copy = part.cloneNode(true) as Element;
      copy.querySelectorAll("button, svg, img[data-slot=site-icon], [aria-hidden=true], [data-copy-skip]").forEach((node) => node.remove());
      for (const node of [copy, ...copy.querySelectorAll("*")]) {
        for (const name of node.getAttributeNames()) if (name !== "href" && name !== "src" && name !== "alt") node.removeAttribute(name);
      }
      return copy.innerHTML;
    })
    .join("");
}
