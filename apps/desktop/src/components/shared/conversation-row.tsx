import { memo, useState, type ComponentType, type MouseEvent } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { NavRow } from "@/components/controls/nav-row";
import { Badge } from "@/components/controls/badge";
import { Archive, BranchAlt, Pencil, Pin, Unarchive, Unpin } from "@/components/icons";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { RowAction, RowActionsSlot } from "@/components/shared/row-action";
import { ProjectBadge, RowCard, RowTime } from "@/components/shared/row-card";
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

/** Keeps what fades at a one-line row's end clear of its two actions while they show. */
const roomForActions =
  "group-hover/row:pe-14 group-focus-within/row:pe-14 group-has-data-[state=open]/row:pe-14";

/** Keeps an archived row's title clear of its one action while it shows. */
const roomForAction =
  "group-hover/row:pe-7 group-focus-within/row:pe-7 group-has-data-[state=open]/row:pe-7";

/** The time gives way to the row's actions; the mark and the chip stay. */
const removeOnRowHover =
  "group-hover/row:hidden group-focus-within/row:hidden group-has-data-[state=open]/row:hidden";

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
 * A chat or session. `twoLine` (Inbox, Pinned): a badge, its project and the time since the user's
 * last message over its title, chip and mark. Otherwise one line: its title, chip, mark and time, or
 * when archived a badge, its title and time. On hover or focus its ⋯ and Pin take the time's place,
 * and a card beside the sidebar tells the rest. A right-click opens its menu (Pin, Rename…, Archive,
 * or Unarchive), or on one of several picked rows the menu for all of them (Archive (N), Unarchive
 * (N)). ⌘-click (Ctrl-click off macOS) picks it, Shift-click picks a range.
 */
export const ConversationRow = memo(function ConversationRow({
  conversation,
  active,
  actions,
  nested = false,
  forked = false,
  twoLine = false,
}: {
  conversation: Conversation;
  active: boolean;
  actions: RowActions;
  nested?: boolean;
  /** Drawn under its source: a little further in, after the fork icon (D211). */
  forked?: boolean;
  twoLine?: boolean;
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
                  !twoLine && (archived ? roomForAction : roomForActions),
                  nested && (forked ? "ps-10" : "ps-8"),
                  !nested && forked && "ps-4",
                  !picked && "group-hover/row:bg-foreground/8",
                  twoLine && "h-auto flex-col items-stretch gap-0.5 py-1",
                )}
                onClick={(event) => {
                  if (!actions.onPick(conversation.id, event)) openConversation(conversation.id);
                }}
              >
                {twoLine ? (
                  <>
                    <span className="flex h-4 min-w-0 items-center gap-1.5">
                      <ProjectBadge name={project} />
                      <span data-slot="row-label" className={cn("text-muted-foreground mask-fade-end min-w-0 flex-1 overflow-hidden text-xs whitespace-nowrap", roomForActions)}>{project ?? "Chat"}</span>
                      <RowTime conversation={conversation} className={removeOnRowHover} />
                    </span>
                    <span className="flex h-5 min-w-0 items-center gap-1.5">
                      {forkIcon}
                      {titleText}
                      <MergedChip conversation={conversation} />
                      {status}
                    </span>
                  </>
                ) : (
                  <>
                    {forkIcon}
                    {archived && <ProjectBadge name={project} />}
                    {titleText}
                    <MergedChip conversation={conversation} />
                    {status}
                    <RowTime conversation={conversation} className={cn("min-w-6.5", removeOnRowHover)} />
                  </>
                )}
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
      <RowActionsSlot className={twoLine ? "top-0.5 bottom-auto h-5" : undefined}>
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
