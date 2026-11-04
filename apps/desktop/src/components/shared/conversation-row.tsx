import { memo, useState, type ComponentType, type MouseEvent } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { NavRow } from "@/components/controls/nav-row";
import { Badge } from "@/components/controls/badge";
import { Archive, BranchAlt, Pencil, Pin, Trash, Unarchive, Unpin } from "@/components/icons";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { RowAction, RowActionsSlot } from "@/components/shared/row-action";
import { SidebarActivityMark, SidebarAmberMark } from "@/components/shared/session-status";
import type { Conversation } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { archive, archiveAll, openConversation, restore, setPinned } from "@/state/actions";
import { askDelete, clearPicked, usePicked } from "@/state/picking";
import { useRowMark, useSettledTabs } from "@/state/tabStates";

/** What the rows can ask the sidebar to do. */
export type RowActions = {
  onRename: (conversation: Conversation) => void;
  onError: (error: string) => void;
  /** A click with ⌘ (Ctrl off macOS) or Shift picks rows; false for a plain click. */
  onPick: (id: string, event: MouseEvent) => boolean;
  /** The picked rows, in the order the sidebar shows them. */
  picked: () => string[];
  /** A row's ⋯ and its menu: the same as its tab's. */
  Menu: ComponentType<{ conversation: Conversation; onRename: () => void }>;
};

/** Keeps a row's title clear of its actions while they show. */
const roomForActions =
  "group-hover/row:pe-14 group-focus-within/row:pe-14 group-has-data-[state=open]/row:pe-14";

/** Hides a row's trailing status while its actions show. */
const hideOnRowHover =
  "group-hover/row:invisible group-focus-within/row:invisible group-has-data-[state=open]/row:invisible";

/** Titles reclaim the mark's space when the row's actions replace it. */
const removeOnRowHover =
  "group-hover/row:hidden group-focus-within/row:hidden group-has-data-[state=open]/row:hidden";

/** A row's trailing mark (spinner, dot or amber ring), with what it means on hover; no state word (D3). */
export function RowStatus({ conversationId, reclaimSpace = false }: { conversationId: string; reclaimSpace?: boolean }) {
  const { look, hover, amber } = useRowMark(conversationId);
  return <span data-slot="row-status" data-status={amber ? "amber" : (look ?? undefined)} title={hover ?? undefined}
    className={cn("empty:hidden flex shrink-0 items-center", reclaimSpace ? removeOnRowHover : hideOnRowHover)}>
    {amber ? <SidebarAmberMark label={hover} /> : <SidebarActivityMark conversationId={conversationId} />}
  </span>;
}

/** Its pull request merged, as the board last saw; an archived row has moved on from that. */
function MergedChip({ conversation }: { conversation: Conversation }) {
  const merged = useSettledTabs((s) => s.byTab[conversation.id]?.prMerged === true);
  if (!merged || conversation.lifecycle === "archived") return null;
  return <Badge data-slot="row-merged" className={cn("bg-pr-purple/15 text-pr-purple", removeOnRowHover)}>Merged</Badge>;
}

/**
 * A chat or session: its title, its state at the end, and on hover its ⋯ and Pin. A
 * right-click opens its menu (Pin, Rename…, Archive, Delete…), or on one of several picked
 * rows the menu for all of them (Archive (N), Delete (N)…). ⌘-click (Ctrl-click off macOS)
 * picks it, Shift-click picks a range.
 */
export const ConversationRow = memo(function ConversationRow({
  conversation,
  active,
  actions,
  nested = false,
  forked = false,
  label,
}: {
  conversation: Conversation;
  active: boolean;
  actions: RowActions;
  nested?: boolean;
  /** Drawn under its source: a little further in, after the fork icon (D211). */
  forked?: boolean;
  /** Small muted line under the title: the inbox's project name (D7). */
  label?: string | undefined;
}) {
  const title = conversation.title;
  const pinned = conversation.pinnedAtMs !== null;
  const archived = conversation.lifecycle === "archived";
  const noun = conversation.kind === "chat" ? "chat" : "session";
  const togglePin = () =>
    void setPinned(conversation.id, !pinned).catch((error: unknown) =>
      actions.onError(errorText(error)),
    );
  const archiveIt = () =>
    void archive(conversation.id).catch((error: unknown) =>
      actions.onError(errorText(error)),
    );
  const unarchiveIt = () =>
    void restore(conversation.id).catch((error: unknown) =>
      actions.onError(errorText(error)),
    );
  const picked = usePicked((s) => s.list === "sidebar" && s.ids.includes(conversation.id));
  const { hover } = useRowMark(conversation.id);
  // What a right-click acts on: every picked row when this is one of several.
  const [targets, setTargets] = useState<string[]>([]);

  return (
    <li data-row-id={conversation.id} className="group/row relative">
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
            title={hover ?? undefined}
            className={cn(
              "data-[selected=true]:bg-link/20 data-[selected=true]:text-foreground pe-1.5",
              roomForActions,
              nested && (forked ? "ps-10" : "ps-8"),
              !nested && forked && "ps-4",
              !picked && "group-hover/row:bg-foreground/8",
              label && "h-auto items-start py-1",
            )}
            onClick={(event) => {
              if (!actions.onPick(conversation.id, event)) openConversation(conversation.id);
            }}
          >
            {forked && <BranchAlt aria-label="Fork" className="text-muted-foreground size-icon-sm" />}
            {/* A title too long for its row fades out at the end; the project keeps its own line. */}
            <span className="flex min-w-0 flex-1 flex-col">
              <span data-slot="row-title" className="mask-fade-end overflow-hidden whitespace-nowrap">{title}</span>
              {label && <span data-slot="row-label" className="text-muted-foreground mask-fade-end overflow-hidden text-xs whitespace-nowrap">{label}</span>}
            </span>
            <MergedChip conversation={conversation} />
            <RowStatus conversationId={conversation.id} reclaimSpace />
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
        ) : archived ? (
          <ContextMenuContent>
            <ContextMenuItem onSelect={unarchiveIt}>
              <Unarchive />
              Unarchive {noun}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem variant="destructive" onSelect={() => askDelete([conversation.id])}>
              <Trash />
              Delete {noun}…
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
        <actions.Menu conversation={conversation} onRename={() => actions.onRename(conversation)} />
        {!archived && (
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
        )}
      </RowActionsSlot>
    </li>
  );
});
