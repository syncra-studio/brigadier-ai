import { Archive, Chats, Folder, Trash, Unarchive } from "@openai/apps-sdk-ui/components/Icon";
import { type MouseEvent, useEffect, useMemo, useRef } from "react";
import { useShallow } from "zustand/react/shallow";

import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
} from "@/app/settings/parts";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import type { Conversation } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";
import { openConversation, restoreAll } from "@/state/actions";
import { askDelete, clearPicked, pickClick, prunePicked, usePicked } from "@/state/picking";
import { useApp } from "@/state/store";

/** The Archived page's rows, for Settings search. */
export const ARCHIVED_ROWS = {
  archived: {
    label: "Archived sessions and chats",
    description: "Hidden from the sidebar and cleaned up; restore one to continue it.",
  },
} as const;

type Group = { key: string; label: string; conversations: Conversation[] };

function useArchivedGroups(): Group[] {
  const conversations = useApp((s) => s.conversations);
  const projects = useApp((s) => s.projects);
  return useMemo(() => {
    const archived = Object.values(conversations)
      .filter((conversation) => conversation.lifecycle === "archived")
      .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs);
    const groups = new Map<string, Group>();
    for (const conversation of archived) {
      const projectId = conversation.kind === "session" ? conversation.projectId : null;
      const key = projectId ?? "chats";
      let group = groups.get(key);
      if (!group) {
        group = {
          key,
          label: projectId ? (projects[projectId]?.name ?? "Project") : "Chats",
          conversations: [],
        };
        groups.set(key, group);
      }
      group.conversations.push(conversation);
    }
    // Projects first (in order of their newest archived session), chats last.
    return [...groups.values()].toSorted(
      (a, b) => Number(a.key === "chats") - Number(b.key === "chats"),
    );
  }, [conversations, projects]);
}

function countLabel(group: Group): string {
  const count = group.conversations.length;
  const noun = group.key === "chats" ? "chat" : "session";
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/**
 * Archived sessions and chats: hidden from the sidebar, cleaned up, and restorable. Their
 * transcripts and artifacts are kept. Rows are picked with ⌘-click (Ctrl-click off macOS) and
 * Shift-click for Unarchive (N) and Delete (N); Delete all deletes every one, after a confirm.
 */
export function ArchivedPage() {
  const groups = useArchivedGroups();
  const mac = useApp((s) => s.info?.platform === "macos");
  const shown = useMemo(
    () => groups.flatMap((group) => group.conversations.map((conversation) => conversation.id)),
    [groups],
  );
  const shownRef = useRef(shown);
  useEffect(() => {
    shownRef.current = shown;
    prunePicked("archived", shown);
  }, [shown]);
  const pickedIds = usePicked(useShallow((s) => (s.list === "archived" ? s.ids : NONE)));
  const picked = shown.filter((id) => pickedIds.includes(id));

  // A title's click opens it; with ⌘ (Ctrl off macOS) or Shift it picks.
  const onTitleClick = (id: string, event: MouseEvent) => {
    if (!pickClick("archived", id, shownRef.current, event, mac)) openConversation(id);
  };

  return (
    <SettingsPage
      title="Archived chats"
      description="Archived sessions and chats keep their transcript and artifacts; their workers, worktrees and processes are gone. Unmerged branches are kept."
      actions={
        shown.length > 0 && (
          <div className="flex items-center gap-2">
            {picked.length > 0 && (
              <>
                <SettingsButton onClick={() => unarchive(picked)}>
                  Unarchive ({picked.length})
                </SettingsButton>
                <SettingsButton destructive onClick={() => askDelete(picked)}>
                  Delete ({picked.length})…
                </SettingsButton>
              </>
            )}
            {picked.length === 0 && (
              <SettingsButton destructive onClick={() => askDelete(shown)}>
                Delete all…
              </SettingsButton>
            )}
          </div>
        )
      }
    >
      {groups.length === 0 && (
        <div className="text-muted-foreground flex flex-col items-center gap-2 py-12 text-sm">
          <Archive aria-hidden className="size-icon-lg" />
          Nothing archived.
        </div>
      )}
      {groups.map((group) => (
        <SettingsSection
          key={group.key}
          title={group.label}
          icon={
            group.key === "chats" ? (
              <Chats aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
            ) : (
              <Folder aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
            )
          }
          actions={<span className="text-muted-foreground text-label">{countLabel(group)}</span>}
        >
          <SettingsCard>
            {group.conversations.map((conversation) => {
              const selected = picked.includes(conversation.id);
              // A right-click acts on every picked row when it is one of them.
              const targets = selected ? picked : [conversation.id];
              return (
                <ContextMenu
                  key={conversation.id}
                  modal={false}
                  onOpenChange={(open) => open && !selected && clearPicked()}
                >
                  <ContextMenuTrigger asChild>
                    <div data-selected={selected} className="data-[selected=true]:bg-link/15">
                      <SettingsRow
                        label={
                          <button
                            type="button"
                            className="max-w-full truncate text-start hover:underline"
                            onClick={(event) => onTitleClick(conversation.id, event)}
                          >
                            {conversation.title}
                          </button>
                        }
                        description={formatDateTime(conversation.updatedAtMs)}
                      >
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={`Delete “${conversation.title}”…`}
                          title="Delete…"
                          className="text-muted-foreground hover:text-destructive"
                          onClick={() => askDelete([conversation.id])}
                        >
                          <Trash />
                        </Button>
                        <SettingsButton onClick={() => unarchive([conversation.id])}>
                          Unarchive
                        </SettingsButton>
                      </SettingsRow>
                    </div>
                  </ContextMenuTrigger>
                  <ContextMenuContent>
                    <ContextMenuItem onSelect={() => unarchive(targets)}>
                      <Unarchive />
                      {targets.length > 1 ? `Unarchive (${targets.length})` : "Unarchive"}
                    </ContextMenuItem>
                    <ContextMenuSeparator />
                    <ContextMenuItem variant="destructive" onSelect={() => askDelete(targets)}>
                      <Trash />
                      {targets.length > 1 ? `Delete (${targets.length})…` : "Delete…"}
                    </ContextMenuItem>
                  </ContextMenuContent>
                </ContextMenu>
              );
            })}
          </SettingsCard>
        </SettingsSection>
      ))}
    </SettingsPage>
  );
}

const NONE: string[] = [];

function unarchive(ids: string[]): void {
  clearPicked();
  void restoreAll(ids);
}
