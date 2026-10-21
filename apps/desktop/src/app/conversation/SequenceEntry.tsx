import { type FC, Suspense } from "react";

import { ImagesRow } from "@/app/conversation/activity/ImageRows";
import { ThreadStep } from "@/app/conversation/activity/Notice";
import { TeamSentence } from "@/app/conversation/activity/TeamSentence";
import { type BlockCard, isImageStep, type SequenceEntry as Entry } from "@/app/conversation/blocks";
import { CompactionRow } from "@/app/conversation/CompactionRow";
import { PeerMessageBlock } from "@/app/conversation/PeerMessageBlock";
import { ReplyText } from "@/app/conversation/ReplyText";
import { cardBody } from "@/app/conversation/threadCode";
import { SteerBubble } from "@/app/conversation/SteerBubble";
import { TimeSeparator } from "@/components/conversation";

/**
 * The card views load on first use, and once loaded render at once: a card in folded work must
 * be whole the frame its work opens, or the row pops in after the fold has measured its height.
 */
export const loadCardBody = cardBody.preload;

export function entryKey(entry: Entry): string {
  switch (entry.kind) {
    case "thinking":
      return `thinking:${entry.segment.itemId}`;
    case "text":
      return `text:${entry.index}`;
    case "card":
      return `${entry.card.type}:${entry.card.id}`;
    case "steer":
      return `steer:${entry.position}`;
    case "orchestrator":
      return `orchestrator:${entry.position}`;
    case "compaction":
      return `compaction:${entry.compaction.id}`;
    case "row":
      return `task:${entry.row.taskId}:${entry.position}`;
    case "gap":
      return `gap:${entry.position}`;
  }
}

function CardEntry({ card }: { card: BlockCard }) {
  return (
    <div data-slot="request-card">
      <Suspense fallback={null}>
        <cardBody.View type={card.type} id={card.id} />
      </Suspense>
    </div>
  );
}

/** A reply, card or row in the block's work. */
export const SequenceEntry: FC<{ entry: Entry; streaming: boolean; commentary?: boolean }> = ({ entry, streaming, commentary = false }) => {
  switch (entry.kind) {
    case "thinking":
      // Thinking is never a row: the live thought feeds the live line.
      return null;
    case "text":
      return (
        <div
          data-slot="aui_assistant-message-content"
          data-commentary={commentary || undefined}
          // The replies Brigadier sent other sessions with this answer land a jump here.
          data-peer-message={entry.anchors?.join(" ")}
          className="text-foreground leading-relaxed wrap-break-word"
        >
          <ReplyText index={entry.index} streaming={streaming} commentary={commentary} />
        </div>
      );
    case "card":
      return <CardEntry card={entry.card} />;
    case "steer":
      // Another session's message steered in, or a reply into its request's turn.
      if (entry.peer) return <PeerMessageBlock peer={entry.peer} text={entry.text} atMs={entry.atMs} />;
      return <SteerBubble text={entry.text} atMs={entry.atMs} attachments={entry.attachments} requestId={entry.requestId} />;
    case "orchestrator":
      if (entry.steps.length > 0 && entry.steps.every(isImageStep)) return <ImagesRow steps={entry.steps} />;
      return <>{entry.steps.map((step) => <ThreadStep key={step.position} step={step} />)}</>;
    case "compaction":
      return <CompactionRow compaction={entry.compaction} />;
    case "row":
      return <TeamSentence row={entry.row} />;
    case "gap":
      return <TimeSeparator atMs={entry.atMs} className="py-2" />;
  }
};
