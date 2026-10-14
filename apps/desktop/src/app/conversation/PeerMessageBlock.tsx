import { useAuiState } from "@assistant-ui/react";
import { Chat, Check, Copy } from "@openai/apps-sdk-ui/components/Icon";
import type { FC } from "react";

import { sentByCaption, UNKNOWN_SESSION } from "@/app/conversation/activity/peerWords";
import type { PeerOf } from "@/app/conversation/blocks";
import { FoldedMarkdown, usePeerName } from "@/app/conversation/peerParts";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import { formatSentAt } from "@/lib/format";
import { openConversation } from "@/state/actions";

/**
 * A message another session sent this one (SESSION-MESSAGING-PLAN.md §6.2), as the user's own
 * stands: right-aligned in the same filled bubble, with over it who sent it ("Sent by API work
 * from another session"), opening the sender at its row; the time and Copy on hover.
 */
export const PeerMessageBlock: FC<{ peer: PeerOf; text: string; atMs: number }> = ({ peer, text, atMs }) => {
  const name = usePeerName(peer.from);
  const shown = name ?? UNKNOWN_SESSION;
  const caption = sentByCaption(shown);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  const words = (
    <>
      <Chat aria-hidden className="size-3.5 shrink-0" />
      <span className="min-w-0 truncate">{caption}</span>
    </>
  );
  return (
    <div
      data-slot="peer-message"
      data-peer-message={peer.messageId}
      role="group"
      aria-label={`Message from ${shown.title}:`}
      className="group/peer flex max-w-7/10 min-w-0 flex-col items-end gap-y-1 self-end"
    >
      {name ? (
        <button
          type="button"
          data-slot="peer-caption"
          onClick={() => openConversation(peer.from, { at: { message: peer.messageId } })}
          className="text-muted-foreground hover:text-foreground rounded-control text-peer-caption flex max-w-full min-w-0 items-center gap-1"
        >
          {words}
        </button>
      ) : (
        <span data-slot="peer-caption" className="text-muted-foreground text-peer-caption flex max-w-full min-w-0 items-center gap-1">
          {words}
        </span>
      )}
      {/* A jump here flashes the message itself, inside its bubble. */}
      <div data-flash className="bg-muted text-foreground rounded-bubble max-w-full min-w-0 px-4 py-2.5">
        <FoldedMarkdown text={text} />
      </div>
      <div className="text-muted-foreground flex items-center gap-1 opacity-0 transition-opacity group-hover/peer:opacity-100 group-focus-within/peer:opacity-100">
        <span className="pe-1 text-xs tabular-nums">{formatSentAt(atMs, now)}</span>
        <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy"} onClick={() => copyToClipboard(text)}>
          {isCopied ? <Check /> : <Copy />}
        </TooltipIconButton>
      </div>
    </div>
  );
};

/** The receiver's block in the user message's place: the thread's slot for a peer anchor. */
export const PeerUserMessage: FC = () => {
  const peer = useAuiState((s) => s.message.metadata.custom["peer"]) as PeerOf | undefined;
  const text = useAuiState((s) =>
    s.message.parts.map((part) => (part.type === "text" ? part.text : "")).join(""),
  );
  const atMs = useAuiState((s) => s.message.createdAt.getTime());
  if (!peer) return null;
  return <PeerMessageBlock peer={peer} text={text} atMs={atMs} />;
};
