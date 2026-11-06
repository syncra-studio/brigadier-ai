import { memo, useState, type ComponentType, type MouseEvent } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { NavRow } from "@/components/controls/nav-row";
import { Badge } from "@/components/controls/badge";
import { Archive, BranchAlt, Pencil, Pin, Unarchive, Unpin } from "@/components/icons";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger, Tooltip, TooltipTrigger } from "@/components/overlays";
import { RowActionsSlot } from "@/components/shared/row-action";
import { ProjectBadge, RowCard } from "@/components/shared/row-card";
import { SidebarRowMark } from "@/components/shared/session-status";
import type { Conversation } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { archive, archiveAll, openConversation, restore, restoreAll, setPinned } from "@/state/actions";
import { clearPicked, usePicked } from "@/state/picking";
import { useApp } from "@/state/store";
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

/** Keeps a row's title and mark clear of its ⋯ while it shows. */
const roomForAction =
  "group-hover/row:pe-7 group-focus-within/row:pe-7 group-has-data-[state=open]/row:pe-7";

/** How long a pointer rests on a row before its card shows; on to the next row it shows at once. */
const CARD_DELAY_MS = 300;

/** A row's trailing mark (spinner, dot, blue or amber ring); `tip` says what it means on hover, where no card does. */
export function RowStatus({ conversationId, tip = true }: { conversationId: string; tip?: boolean }) {
  const { look, hover, waiting, amber } = useRowMark(conversationId);
  return <span data-slot="row-status" data-status={waiting ? "waiting" : amber ? "amber" : (look ?? undefined)} title={tip ? (hover ?? undefined) : undefined}
    className="empty:hidden flex shrink-0 items-center">
    <SidebarRowMark conversationId={conversationId} />
  </span>;
}

/** Its pull request merged, as the board last saw; an archived row has moved on from that. */
function MergedChip({ conversation }: { conversation: Conversation }) {
  const merged = useSettledTabs((s) => s.byTab[conversation.id]?.prMerged === true);
  if (!merged || conversation.lifecycle === "archived") return null;
  return <Badge data-slot="row-merged" className="bg-pr-purple/15 text-pr-purple me-0.5">Merged</Badge>;
}

const isArchived = (id: string) => useApp.getState().conversations[id]?.lifecycle === "archived";

/**
 * A chat or session on one line: its title, chip and mark; `badged` (the one list, and Pinned), its
 * project's badge before them and more height. On hover or focus its ⋯ shows, and a card beside the
 * sidebar tells the rest. A right-click opens its menu (Pin, Rename…, Archive, or Unarchive), or on
 * one of several picked rows the menu for all of them (Archive (N), Unarchive (N)). ⌘-click
 * (Ctrl-click off macOS) picks it, Shift-click picks a range.
 */
export const ConversationRow = memo(function ConversationRow({
  conversation,
  active,
  actions,
  nested = false,
  forked = false,
  badged = false,
}: {
  conversation: Conversation;
  active: boolean;
  actions: RowActions;
  nested?: boolean;
  /** Drawn under its source: a little further in, after the fork icon (D211). */
  forked?: boolean;
  badged?: boolean;
}) {
  const title = conversation.title;
  const pinned = conversation.pinnedAtMs !== null;
  const archived = conversation.lifecycle === "archived";
  const noun = conversation.kind === "chat" ? "chat" : "session";
  // Null for a chat, as for a session whose project is gone.
  const project = useApp((s) => (conversation.kind !== "chat" && conversation.projectId ? (s.projects[conversation.projectId]?.name ?? null) : null));
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
  // What a right-click acts on: every picked row when this is one of several.
  const [targets, setTargets] = useState<string[]>([]);
  const liveTargets = targets.filter((id) => !isArchived(id));
  const archivedTargets = targets.filter(isArchived);
  const forkIcon = forked && <BranchAlt aria-label="Fork" className="text-muted-foreground size-icon-sm" />;
  const titleText = <span data-slot="row-title" className={cn("mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap", archived && "text-foreground/70")}>{title}</span>;
  const status = <RowStatus conversationId={conversation.id} tip={false} />;

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
        <Tooltip delayDuration={CARD_DELAY_MS}>
          <ContextMenuTrigger asChild>
            <TooltipTrigger asChild>
              <NavRow
                data-active={active}
                data-selected={picked}
                aria-current={active ? "page" : undefined}
                className={cn(
                  "data-[selected=true]:bg-link/20 data-[selected=true]:text-foreground pe-1.5",
                  roomForAction,
                  nested && (forked ? "ps-10" : "ps-8"),
                  !nested && forked && "ps-4",
                  !picked && "group-hover/row:bg-foreground/8",
                  badged && "h-9",
                )}
                onClick={(event) => {
                  if (!actions.onPick(conversation.id, event)) openConversation(conversation.id);
                }}
              >
                {forkIcon}
                {badged && <ProjectBadge name={project} projectId={conversation.projectId} />}
                {titleText}
                <MergedChip conversation={conversation} />
                {status}
              </NavRow>
            </TooltipTrigger>
          </ContextMenuTrigger>
          <RowCard conversation={conversation} project={project} />
        </Tooltip>
        {targets.length > 1 ? (
          <ContextMenuContent>
            {liveTargets.length > 0 && (
              <ContextMenuItem
                onSelect={() => {
                  clearPicked();
                  void archiveAll(liveTargets);
                }}
              >
                <Archive />
                Archive ({liveTargets.length})
              </ContextMenuItem>
            )}
            {archivedTargets.length > 0 && (
              <ContextMenuItem
                onSelect={() => {
                  clearPicked();
                  void restoreAll(archivedTargets).catch((error: unknown) => actions.onError(errorText(error)));
                }}
              >
                <Unarchive />
                Unarchive ({archivedTargets.length})
              </ContextMenuItem>
            )}
          </ContextMenuContent>
        ) : archived ? (
          <ContextMenuContent>
            <ContextMenuItem onSelect={unarchiveIt}>
              <Unarchive />
              Unarchive {noun}
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
          </ContextMenuContent>
        )}
      </ContextMenu>
      <RowActionsSlot>
        <actions.Menu conversation={conversation} onRename={() => actions.onRename(conversation)} />
      </RowActionsSlot>
    </li>
  );
});
