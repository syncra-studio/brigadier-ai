import { useAui, useAuiState } from "@assistant-ui/react";
import {
  ArrowCurvedRight,
  DotsHorizontal,
  Pencil,
  Play,
  Trash,
} from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useContext } from "react";

import { InlineImageText } from "@/app/conversation/InlineImage";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { useAction } from "@/app/conversation/useAction";
import { ComposerRailItem } from "@/components/assistant-ui/elements/composer-rail";
import {
  MessageQueue,
  MessageQueueItem,
  MessageQueuePaused,
} from "@/components/assistant-ui/elements/message-queue";
import { ghostButton } from "@/components/assistant-ui/elements/surfaces";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type { AttachmentRef, QueuedMessage } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { deleteQueued, resumeQueue } from "@/state/actions";
import { useBoard } from "@/state/board";

const NO_ITEMS: QueuedMessage[] = [];

function useQueueItems(conversationId: string | null): QueuedMessage[] {
  return useBoard((s) =>
    conversationId && s.board?.conversationId === conversationId ? s.board.queue.items : NO_ITEMS,
  );
}

/**
 * Pulls a queued message into the empty composer ("Edit message", and ↑ on an empty
 * composer): the row leaves the queue, its text and attachments fill the composer, and
 * sending puts it back in the same slot. `null` while there is nothing to pull.
 */
export function usePullQueued(): ((index: number) => Promise<void>) | null {
  const aui = useAui();
  const target = useContext(ComposerTargetContext);
  const conversationId = target?.conversation?.id ?? null;
  const items = useQueueItems(conversationId);
  const pull = useCallback(
    async (index: number) => {
      // A negative index counts from the end (-1: the last one, for ↑).
      const at = index < 0 ? items.length + index : index;
      const item = items[at];
      const composer = aui.composer();
      if (!target || !conversationId || !item || !composer.getState().isEmpty) return;
      await deleteQueued(conversationId, item.id);
      target.queue.pulled.set(conversationId, at, item.mentions);
      target.mentions.recall(item.mentions);
      target.queue.attachments.inline.recall(item.text, item.attachments);
      composer.setText(item.text);
      for (const ref of item.attachments.filter((attachment) => !attachment.inline)) {
        await composer.addAttachment(target.queue.attachments.adopt(ref));
      }
    },
    [aui, target, conversationId, items],
  );
  return conversationId && items.length > 0 ? pull : null;
}

/** An attachment-only message's label, or what rides along with the text. */
function attachmentLabel(attachments: readonly AttachmentRef[]): string | null {
  if (attachments.length === 0) return null;
  const images = attachments.every((attachment) => attachment.mime.startsWith("image/"));
  const [only] = attachments;
  if (attachments.length === 1 && only) {
    return images ? "Image attachment" : only.pasted ? "Pasted text" : only.name;
  }
  return `${attachments.length} ${images ? "images" : "files"}`;
}

/**
 * The queue card on the composer rail: what waits for the running turn, one row each (glyph,
 * text, ↳ Steer, delete, ⋯), dragged by the grip that replaces the glyph on hover.
 * Steering, moving and deleting go through assistant-ui's queue (the view's adapter talks to
 * the daemon). After an interrupt the queue pauses until resumed.
 */
export function QueueCard({ conversationId }: { conversationId: string }) {
  const aui = useAui();
  const queue = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.queue : null));
  const items = queue?.items ?? NO_ITEMS;
  // Behind the row now `to - 1` (the front for 0), in assistant-ui's terms.
  const moveTo = (id: string, to: number) => {
    const rest = items.filter((item) => item.id !== id);
    aui.composer().queueItem({ id }).move({ lane: "queue", insertAfter: rest[to - 1]?.id ?? null });
  };
  const { listRef, shown, dragging, grip } = useDragReorder<QueuedMessage, HTMLOListElement>({
    items,
    idOf: (item) => item.id,
    rowSelector: "[data-slot=message-queue-item]",
    onMove: moveTo,
  });
  const pull = usePullQueued();
  // Pulling a message fills the composer, so only into an empty one.
  const composerEmpty = useAuiState((s) => s.composer.isEmpty);
  const action = useAction();
  if (items.length === 0) return null;

  return (
    <ComposerRailItem label="Message queue">
      {queue?.paused && (
        <MessageQueuePaused>
          <Button
            size="xs"
            variant="ghost"
            onClick={() => action.run(() => resumeQueue(conversationId))}
            disabled={action.busy}
          >
            <Play />
            Resume
          </Button>
        </MessageQueuePaused>
      )}
      <MessageQueue ref={listRef} aria-label="Queued messages">
        {shown.map((item, index) => {
          const attached = attachmentLabel(item.attachments.filter((attachment) => !attachment.inline));
          return (
            <MessageQueueItem
              key={item.id}
              dragging={dragging === item.id}
              deciding={item.deciding}
              grip={
                items.length > 1 && (
                  <button
                    type="button"
                    aria-label="Drag to reorder"
                    title="Drag to reorder"
                    className={cn(ghostButton, "size-full cursor-grab touch-none active:cursor-grabbing")}
                    {...grip(item.id, index)}
                  >
                    <DotsHorizontal className="size-icon-sm rotate-90" />
                  </button>
                )
              }
            >
              <span className="min-w-0 flex-1 truncate" title={item.text || undefined}>
                {item.text ? (
                  <>
                    <InlineImageText text={item.text} attachments={item.attachments} />
                    {attached && <span className="text-muted-foreground"> · {attached}</span>}
                  </>
                ) : (
                  <span className="text-muted-foreground">{attached}</span>
                )}
              </span>
              {item.deciding && (
                <span
                  title="Brigadier is checking whether this belongs to the answer in progress"
                  className="text-muted-foreground shimmer shrink-0 text-xs motion-reduce:animate-none"
                >
                  Deciding
                </span>
              )}
              <TooltipIconButton
                tooltip="Submit without interrupting the model"
                side="top"
                size="xs"
                className="text-muted-foreground w-auto"
                onClick={() => aui.composer().queueItem({ id: item.id }).move({ lane: "steer", insertAfter: null })}
              >
                <ArrowCurvedRight />
                Steer
              </TooltipIconButton>
              <TooltipIconButton
                tooltip="Delete queued message"
                side="top"
                className="text-muted-foreground"
                onClick={() => aui.composer().queueItem({ id: item.id }).remove()}
              >
                <Trash />
              </TooltipIconButton>
              <DropdownMenu modal={false}>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label="Queued message actions"
                    className="text-muted-foreground"
                  >
                    <DotsHorizontal />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem
                    disabled={!pull || !composerEmpty}
                    onSelect={() => action.run(async () => pull?.(items.indexOf(item)))}
                  >
                    <Pencil />
                    Edit message
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </MessageQueueItem>
          );
        })}
      </MessageQueue>
      {action.error && (
        <p role="alert" className="text-destructive px-3 pb-1 text-xs">
          {action.error}
        </p>
      )}
    </ComposerRailItem>
  );
}
