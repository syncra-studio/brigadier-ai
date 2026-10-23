import { memo, useState, type MouseEvent } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { NavRow } from "@/components/controls/nav-row";
import { Archive, Pencil, Pin, Trash, Unpin } from "@/components/icons";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { RowAction, RowActionsSlot } from "@/components/shared/row-action";
import { SessionStatusLabel, SidebarActivityMark, SidebarError } from "@/components/shared/session-status";
import type { Conversation } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { archive, archiveAll, openConversation, setPinned } from "@/state/actions";
import { lastFinished, unread, useLastOpened } from "@/state/lastOpened";
import { askDelete, clearPicked, usePicked } from "@/state/picking";
import { useApp } from "@/state/store";

/** What the rows can ask the sidebar to do. */
export type RowActions = {
  onRename: (conversation: Conversation) => void;
  onError: (error: string) => void;
  /** A click with ⌘ (Ctrl off macOS) or Shift picks rows; false for a plain click. */
  onPick: (id: string, event: MouseEvent) => boolean;
  /** The picked rows, in the order the sidebar shows them. */
  picked: () => string[];
};

/** Keeps a row's title clear of its actions while they show. */
const roomForActions =
  "group-hover/row:pe-14 group-focus-within/row:pe-14 group-has-data-[state=open]/row:pe-14";

/** A title beside its row's state word: cut with an ellipsis, not faded into the word. */
const besideStatus = "group-has-data-[slot=status-word]/row:mask-none group-has-data-[slot=status-word]/row:text-ellipsis";

/** Hides a row's trailing status while its actions show. */
const hideOnRowHover =
  "group-hover/row:invisible group-focus-within/row:invisible group-has-data-[state=open]/row:invisible";

/** A row's state word and mark, with its reason on hover; otherwise its unread dot. */
export function RowStatus({ conversationId }: { conversationId: string }) {
  const seen = useLastOpened((s) => s.at[conversationId]);
  const finishedAtMs = useApp((s) => lastFinished(s.conversations[conversationId]));
  const open = useApp((s) => s.selection.type === "conversation" && s.selection.id === conversationId);
  const isUnread = unread(finishedAtMs, seen, open);
  const mark = <SidebarActivityMark conversationId={conversationId} unread={isUnread} />;
  return <SessionStatusLabel conversationId={conversationId} className={hideOnRowHover}
    rowIndicator={mark} fallback={isUnread ? <span data-slot="row-status" data-status="unread" className={hideOnRowHover}>{mark}</span> : null} />;
}

/**
 * A chat or session: its title, its state at the end, and on hover Pin and Archive. A
 * right-click opens its menu (Pin, Rename…, Archive, Delete…), or on one of several picked
 * rows the menu for all of them (Archive (N), Delete (N)…). ⌘-click (Ctrl-click off macOS)
 * picks it, Shift-click picks a range.
 */
export const ConversationRow = memo(function ConversationRow({
  conversation,
  active,
  actions,
  nested = false,
}: {
  conversation: Conversation;
  active: boolean;
  actions: RowActions;
  nested?: boolean;
}) {
  const title = conversation.title;
  const pinned = conversation.pinnedAtMs !== null;
  const noun = conversation.kind === "chat" ? "chat" : "session";
  const togglePin = () =>
    void setPinned(conversation.id, !pinned).catch((error: unknown) =>
      actions.onError(errorText(error)),
    );
  const archiveIt = () =>
    void archive(conversation.id).catch((error: unknown) =>
      actions.onError(errorText(error)),
    );
  const picked = usePicked((s) => s.list === "sidebar" && s.ids.includes(conversation.id));
  // What a right-click acts on: every picked row when this is one of several.
  const [targets, setTargets] = useState<string[]>([]);

  return (
    <li className="group/row relative">
      <ContextMenu
        modal={false}
        onOpenChange={(open) => {
          if (!open) return;
          const all = actions.picked();
          if (all.includes(conversation.id)) setTargets(all);
          else {
            // A right-click on a row not picked is about that row alone.
            clearPicked();
            setTargets([conversation.id]);
          }
        }}
      >
        <ContextMenuTrigger asChild>
          <NavRow
            data-active={active}
            data-selected={picked}
            aria-current={active ? "page" : undefined}
            className={cn(
              "data-[selected=true]:bg-link/20 data-[selected=true]:text-foreground pe-1.5",
              roomForActions,
              nested && "ps-8",
            )}
            onClick={(event) => {
              if (!actions.onPick(conversation.id, event)) openConversation(conversation.id);
            }}
          >
            <SidebarError conversationId={conversation.id} />
            {/* A title too long for its row fades out at the end; beside a state word it ends in "…"
                 instead, so the word never reads as covering it. */}
            <span data-slot="row-title" className={cn("mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap", besideStatus)}>
              {title}
            </span>
            <RowStatus conversationId={conversation.id} />
          </NavRow>
        </ContextMenuTrigger>
        {targets.length > 1 ? (
          <ContextMenuContent>
            <ContextMenuItem
              onSelect={() => {
                clearPicked();
                void archiveAll(targets);
              }}
            >
              <Archive />
              Archive ({targets.length})
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem variant="destructive" onSelect={() => askDelete(targets)}>
              <Trash />
              Delete ({targets.length})…
            </ContextMenuItem>
          </ContextMenuContent>
        ) : (
          <ContextMenuContent>
            <ContextMenuItem onSelect={togglePin}>
              {pinned ? <Unpin /> : <Pin />}
              {pinned ? `Unpin ${noun}` : `Pin ${noun}`}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => actions.onRename(conversation)}>
              <Pencil />
              Rename {noun}…
            </ContextMenuItem>
            <ContextMenuItem onSelect={archiveIt}>
              <Archive />
              Archive {noun}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem variant="destructive" onSelect={() => askDelete([conversation.id])}>
              <Trash />
              Delete {noun}…
            </ContextMenuItem>
          </ContextMenuContent>
        )}
      </ContextMenu>
      <RowActionsSlot>
        <Tooltip>
          <TooltipTrigger asChild>
            <RowAction
              aria-label={`${pinned ? "Unpin" : "Pin"} ${title}`}
              onClick={togglePin}
            >
              {pinned ? <Unpin /> : <Pin />}
            </RowAction>
          </TooltipTrigger>
          <TooltipContent side="bottom">{pinned ? `Unpin ${noun}` : `Pin ${noun}`}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <RowAction
              aria-label={`Archive ${title}`}
              onClick={archiveIt}
            >
              <Archive />
            </RowAction>
          </TooltipTrigger>
          <TooltipContent side="bottom">Archive {noun}</TooltipContent>
        </Tooltip>
      </RowActionsSlot>
    </li>
  );
});
