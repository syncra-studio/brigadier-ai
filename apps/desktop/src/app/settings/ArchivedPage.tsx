import { type MouseEvent, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import {
  ALL_ARCHIVED,
  type ArchivedFilter,
  archivedGroups,
  archivedOf,
  groupCount,
  KIND_CHOICES,
  narrowed,
  projectChoices,
  shownIds,
  SORT_CHOICES,
} from "@/app/settings/archived";
import { SettingsPage, SettingsSelect, SettingsSelectTrigger } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/overlays";
import { Branch, Chats, Check, DotsHorizontal, Filter, Folder, Trash, Unarchive } from "@/components/icons";
import { EmptyState, SettingsCard, SettingsSearch } from "@/components/shared";
import type { Conversation } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";
import { openConversation, restoreAll } from "@/state/actions";
import { askDelete, clearPicked, pickClick, prunePicked, usePicked } from "@/state/picking";
import { useApp } from "@/state/store";

/** The Archived page's rows, for Settings search. */
export const ARCHIVED_ROWS = {
  archived: {
    label: "Archived sessions and chats",
    description: "Hidden from the sidebar and cleaned up; restore one to continue it",
  },
} as const;

/**
 * Archived sessions and chats: a search and two menus (kind and order, project) over the list,
 * which is grouped by project, chats last. Each row opens on its title and has Unarchive and
 * Delete. Rows are also picked with ⌘-click (Ctrl-click off macOS) and Shift-click for
 * Unarchive (N) and Delete (N)…, which show only while rows are picked; Delete all… is in the
 * toolbar's "…" menu.
 */
export function ArchivedPage() {
  const conversations = useApp((s) => s.conversations);
  const projects = useApp((s) => s.projects);
  const mac = useApp((s) => s.info?.platform === "macos");
  const [filter, setFilter] = useState<ArchivedFilter>(ALL_ARCHIVED);

  const archived = useMemo(() => archivedOf(conversations), [conversations]);
  const projectMenu = useMemo(() => projectChoices(archived, projects), [archived, projects]);
  // A project chosen in the menu whose last archived session left shows them all again.
  const effective = useMemo(
    () =>
      projectMenu.some((choice) => choice.value === filter.project)
        ? filter
        : { ...filter, project: "all" },
    [projectMenu, filter],
  );
  const groups = useMemo(
    () => archivedGroups(archived, projects, effective),
    [archived, projects, effective],
  );
  const shown = useMemo(() => shownIds(groups), [groups]);

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

  const filtered = narrowed(effective);
  return (
    <SettingsPage title="Archived chats">
      {/* The toolbar stays in view over the list; the groups follow it. */}
      <div className="flex flex-col">
        {archived.length > 0 && (
          <div className="bg-background sticky top-0 z-10 -mt-5 flex flex-col gap-3 pt-5 pb-2">
            <div className="flex items-center gap-2">
              <SettingsSearch
                label="Search archived chats"
                value={filter.query}
                onChange={(query) => setFilter((f) => ({ ...f, query }))}
              />
              <KindMenu filter={effective} onChange={setFilter} />
              <ProjectMenu
                filter={effective}
                choices={projectMenu}
                onChange={(project) => setFilter((f) => ({ ...f, project }))}
              />
              <MoreMenu
                count={shown.length}
                filtered={filtered}
                onDeleteAll={() => askDelete(shown)}
              />
            </div>
            {picked.length > 0 && (
              <div
                role="toolbar"
                aria-label="Picked chats"
                className="flex items-center gap-2"
              >
                <span className="text-foreground/65 text-label flex-1">
                  {picked.length} picked
                </span>
                <Button variant="soft" size="sm" onClick={() => clearPicked()}>Clear</Button>
                <Button variant="soft" size="sm" onClick={() => unarchive(picked)}>
                  Unarchive ({picked.length})
                </Button>
                <Button variant="soft-destructive" size="sm" onClick={() => askDelete(picked)}>
                  Delete ({picked.length})…
                </Button>
              </div>
            )}
          </div>
        )}

        {shown.length === 0 && (
          <EmptyState variant="centered" title={archived.length === 0 ? "No archived chats" : "No archived chats match"} />
        )}

        <div className="flex flex-col gap-10 pt-5">
        {groups.map((group) => (
          <section key={group.key} aria-label={group.label} className="flex flex-col gap-1.5">
            <div className="flex items-center gap-4 pb-1.5">
              <h2 className="flex min-w-0 flex-1 items-center gap-2 text-sm font-medium">
                {group.key === "chats" ? (
                  <Chats aria-hidden className="size-icon-md shrink-0" />
                ) : (
                  <Folder aria-hidden className="size-icon-md shrink-0" />
                )}
                <span className="truncate">{group.label}</span>
              </h2>
              <span className="text-foreground/65 text-label shrink-0">{groupCount(group)}</span>
            </div>
            <SettingsCard>
              {group.conversations.map((conversation) => (
                <ArchivedRow
                  key={conversation.id}
                  conversation={conversation}
                  picked={picked}
                  onTitleClick={onTitleClick}
                />
              ))}
            </SettingsCard>
          </section>
        ))}
        </div>
      </div>
    </SettingsPage>
  );
}

const NONE: string[] = [];

function unarchive(ids: string[]): void {
  clearPicked();
  void restoreAll(ids);
}

/** One archived conversation: what it is, its title (which opens it) and date, its two actions. */
function ArchivedRow({
  conversation,
  picked,
  onTitleClick,
}: {
  conversation: Conversation;
  picked: string[];
  onTitleClick: (id: string, event: MouseEvent) => void;
}) {
  const selected = picked.includes(conversation.id);
  // A right-click acts on every picked row when it is one of them.
  const targets = selected ? picked : [conversation.id];
  const Icon = conversation.kind === "session" ? Branch : Chats;
  return (
    <ContextMenu modal={false} onOpenChange={(open) => open && !selected && clearPicked()}>
      <ContextMenuTrigger asChild>
        <div
          data-slot="archived-row"
          data-selected={selected}
          className="data-[selected=true]:bg-link/15 flex items-center gap-6 px-4 py-3"
        >
          <div className="flex min-w-0 flex-1 items-center gap-3">
            <Icon
              aria-label={conversation.kind === "session" ? "Session" : "Chat"}
              className="text-foreground/65 size-icon-md shrink-0"
            />
            <div className="flex min-w-0 flex-col gap-0.5">
              <button
                type="button"
                className="text-link text-label max-w-full truncate text-start font-medium hover:underline"
                onClick={(event) => onTitleClick(conversation.id, event)}
              >
                {conversation.title}
              </button>
              <span className="text-foreground/65 text-xs">
                {formatDateTime(conversation.updatedAtMs)}
              </span>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Button variant="soft" size="sm" onClick={() => unarchive([conversation.id])}>
              Unarchive
            </Button>
            <Button
              variant="soft-destructive"
              size="sm"
              aria-label={`Delete “${conversation.title}”…`}
              onClick={() => askDelete([conversation.id])}
            >
              Delete
            </Button>
          </div>
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
}

const check = <Check className="size-icon-md" />;

/** Which kind shows (all, sessions, chats) and in what order. */
function KindMenu({
  filter,
  onChange,
}: {
  filter: ArchivedFilter;
  onChange: (update: (filter: ArchivedFilter) => ArchivedFilter) => void;
}) {
  const kind = KIND_CHOICES.find((choice) => choice.value === filter.kind)?.label;
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <SettingsSelectTrigger
          label="Filter archived chats"
          icon={<Filter aria-hidden className="size-icon-sm shrink-0" />}
          className="w-40"
        >
          {kind}
        </SettingsSelectTrigger>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-(--radix-dropdown-menu-trigger-width)">
        <DropdownMenuLabel>Type</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={filter.kind}
          onValueChange={(value) => onChange((f) => ({ ...f, kind: value as ArchivedFilter["kind"] }))}
        >
          {KIND_CHOICES.map((choice) => (
            <DropdownMenuRadioItem key={choice.value} value={choice.value} indicator={check}>
              {choice.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Sort by</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={filter.sort}
          onValueChange={(value) => onChange((f) => ({ ...f, sort: value as ArchivedFilter["sort"] }))}
        >
          {SORT_CHOICES.map((choice) => (
            <DropdownMenuRadioItem key={choice.value} value={choice.value} indicator={check}>
              {choice.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Which project's sessions show, or the chats. */
function ProjectMenu({
  filter,
  choices,
  onChange,
}: {
  filter: ArchivedFilter;
  choices: { value: string; label: string }[];
  onChange: (project: string) => void;
}) {
  return (
    <SettingsSelect
      label="Filter archived chats by project"
      value={filter.project}
      options={choices}
      onChange={onChange}
      icon={<Folder aria-hidden className="size-icon-sm shrink-0" />}
      className="w-40"
    />
  );
}

/** "…": Delete all (of what shows). */
function MoreMenu({
  count,
  filtered,
  onDeleteAll,
}: {
  count: number;
  filtered: boolean;
  onDeleteAll: () => void;
}) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="More"
          className="text-foreground/65 shrink-0"
        >
          <DotsHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem variant="destructive" disabled={count === 0} onSelect={onDeleteAll}>
          <Trash />
          {filtered ? `Delete these ${count}…` : "Delete all…"}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
