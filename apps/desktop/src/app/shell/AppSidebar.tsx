import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { TooltipIconButton, Button, Kbd, NavRow } from "@/components/controls";
import { errorText } from "@/app/shell/dialogs/fields";
import { ProjectDialog } from "@/app/shell/dialogs/ProjectDialog";
import { RemoveProjectDialog } from "@/app/shell/dialogs/RemoveProjectDialog";
import { NameDialog } from "@/app/shell/NameDialog";
import { useShortcuts } from "@/app/shortcuts";
import { openSearch } from "@/app/shell/SearchDialog";
import { besideStrip } from "@/app/shell/sidebar/nav";
import {
  BrigadierGlyph,
  Chats,
  ComposeEditSquare,
  DotsHorizontal,
  Download,
  Folder,
  FolderOpen,
  Folders,
  MagnifyingGlassSearch,
  Plus,
  SettingsCog,
  Sidebar,
  Trash,
} from "@/components/icons";
import { Banner, ConversationRow, EmptyState, NavFold, NavHeader, NavList, NavSection, RowAction, RowActionsSlot, RowStatus, SidebarError, type RowActions } from "@/components/shared";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger, Popover, PopoverContent, PopoverTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { useSidebar } from "@/components/layout";
import { openFolder } from "@/ipc/client";
import type { Conversation, Project } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  openConversation,
  renameConversation,
  select,
  setProjectExpanded,
} from "@/state/actions";
import { openAddProject } from "@/state/addProject";
import { exportProjectConventions } from "@/state/brain";
import { pickClick, pickedIn, prunePicked } from "@/state/picking";
import { useTabStates } from "@/state/tabStates";
import { useApp } from "@/state/store";

// ----- the Home panel --------------------------------------------------------------------

type DialogState =
  | { type: "projectSettings"; project: Project }
  | { type: "rename"; conversation: Conversation }
  | null;

type Sections = {
  pinned: Conversation[];
  chats: Conversation[];
  projects: Project[];
  sessions: Record<string, Conversation[]>;
};

function useSections(): Sections {
  const projects = useApp((s) => s.projects);
  const conversations = useApp((s) => s.conversations);
  return useMemo(() => {
    // Archived conversations live in Settings → Archived chats only.
    // Side chats are temporary, shown only beside their conversation.
    const all = Object.values(conversations).filter(
      (conversation) => conversation.lifecycle !== "archived" && !conversation.sideOf,
    );
    const pinned = all
      .filter((conversation) => conversation.pinnedAtMs !== null)
      .toSorted((a, b) => (b.pinnedAtMs ?? 0) - (a.pinnedAtMs ?? 0));
    const recent = all
      .filter((conversation) => conversation.pinnedAtMs === null)
      .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs);
    const sessions: Record<string, Conversation[]> = {};
    const chats: Conversation[] = [];
    for (const conversation of recent) {
      if (conversation.kind === "chat" || !conversation.projectId) {
        chats.push(conversation);
      } else {
        (sessions[conversation.projectId] ??= []).push(conversation);
      }
    }
    // Projects with recent activity first, then newest.
    const lastActivity = (project: Project) =>
      Math.max(project.createdAtMs, sessions[project.id]?.[0]?.updatedAtMs ?? 0);
    const sortedProjects = Object.values(projects).toSorted(
      (a, b) => lastActivity(b) - lastActivity(a),
    );
    return { pinned, chats, projects: sortedProjects, sessions };
  }, [projects, conversations]);
}

/**
 * The light inbox (Q63): each project's sessions that need the user come first, in their order
 * otherwise.
 */
function useInboxSections(): Sections {
  const sections = useSections();
  const { projects, sessions } = sections;
  const ids = useMemo(
    () => projects.flatMap((project) => (sessions[project.id] ?? NO_SESSIONS).map((conversation) => conversation.id)),
    [projects, sessions],
  );
  const needs = useTabStates(ids)
    .map((state) => (state === "needsYou" ? "1" : "0"))
    .join("");
  return useMemo(() => {
    const needing = new Set(ids.filter((_, index) => needs[index] === "1"));
    const ordered: Record<string, Conversation[]> = {};
    for (const [projectId, list] of Object.entries(sessions)) {
      const first = list.filter((conversation) => needing.has(conversation.id));
      ordered[projectId] =
        first.length > 0 ? [...first, ...list.filter((conversation) => !needing.has(conversation.id))] : list;
    }
    return { ...sections, sessions: ordered };
  }, [sections, sessions, ids, needs]);
}

type SectionId = "pinned" | "projects" | "chats";
const SECTIONS_KEY = "brigadier.sidebarSections";

function cachedFolded(): SectionId[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SECTIONS_KEY) ?? "[]");
    return Array.isArray(value) ? value.filter((id): id is SectionId => typeof id === "string") : [];
  } catch {
    return [];
  }
}

/** Which sections are folded away, remembered across launches. */
function useFoldedSections() {
  const [folded, setFolded] = useState(cachedFolded);
  const setOpen = (id: SectionId, open: boolean) => {
    const next = open ? folded.filter((entry) => entry !== id) : [...folded, id];
    setFolded(next);
    try {
      localStorage.setItem(SECTIONS_KEY, JSON.stringify(next));
    } catch {
      // Storage can be unavailable; the sections then open again on the next launch.
    }
  };
  return {
    folded,
    isOpen: (id: SectionId) => !folded.includes(id),
    setOpen,
  };
}

/**
 * The sidebar panel on Home: its header (title and search), New chat, then Pinned (when there
 * are any), Projects with their sessions, and Chats. Each section folds away from its title.
 */
export function AppSidebar() {
  const { pinned, chats, projects, sessions } = useInboxSections();
  const activeId = useApp((s) => (s.selection.type === "conversation" ? s.selection.id : null));
  const draftProjectId = useApp((s) =>
    s.selection.type === "draft" && s.selection.kind === "session" ? s.selection.projectId : null,
  );
  const isChatDraft = useApp((s) => s.selection.type === "draft" && s.selection.kind === "chat");
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const shortcuts = useShortcuts();
  const sections = useFoldedSections();
  const [dialog, setDialog] = useState<DialogState>(null);
  const [removing, setRemoving] = useState<Project | null>(null);
  const [error, setError] = useState<string | null>(null);
  const expandedProjects = useApp((s) => s.expandedProjects);
  const { folded } = sections;

  // The rows as they show, top to bottom: what a Shift-click picks a range of.
  const shown = useMemo(() => {
    const rows: string[] = [];
    const isOpen = (id: SectionId) => !folded.includes(id);
    if (isOpen("pinned")) rows.push(...pinned.map((c) => c.id));
    if (isOpen("projects")) {
      for (const project of projects) {
        if (expandedProjects[project.id] ?? true) {
          rows.push(...(sessions[project.id] ?? NO_SESSIONS).map((c) => c.id));
        }
      }
    }
    if (isOpen("chats")) rows.push(...chats.map((c) => c.id));
    return rows;
  }, [pinned, projects, sessions, chats, expandedProjects, folded]);
  const shownRef = useRef(shown);
  useEffect(() => {
    shownRef.current = shown;
    prunePicked("sidebar", shown);
  }, [shown]);

  const actions = useMemo<RowActions>(
    () => ({
      onRename: (conversation) => setDialog({ type: "rename", conversation }),
      onError: (message) => setError(message),
      onPick: (id, event) => {
        const { info, selection } = useApp.getState();
        const open = selection.type === "conversation" ? selection.id : null;
        return pickClick("sidebar", id, shownRef.current, event, info?.platform === "macos", open);
      },
      picked: () => pickedIn("sidebar", shownRef.current),
    }),
    [],
  );
  const onProjectSettings = (project: Project) => setDialog({ type: "projectSettings", project });

  return (
    <>
      <NavHeader
        title={
          <span className="-ms-0.5 flex items-center gap-2">
            <BrigadierGlyph className="size-5 shrink-0" />
            Brigadier
          </span>
        }
        actions={
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-md"
                aria-label="Search"
                className="text-muted-foreground hover:text-foreground"
                onClick={() => openSearch()}
              >
                <MagnifyingGlassSearch />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">
              Search
              <Kbd>{shortcuts.search}</Kbd>
            </TooltipContent>
          </Tooltip>
        }
      />
      <NavList className="mb-2 px-2">
        <li>
          <NavRow
            data-active={isChatDraft}
            onClick={() => select({ type: "draft", kind: "chat" })}
          >
            <ComposeEditSquare />
            <span className="truncate">New chat</span>
          </NavRow>
        </li>
      </NavList>

      <div className="scroll-edge-fade flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-2 pt-1 pb-2">
        {pinned.length > 0 && (
          <NavSection
            title="Pinned"
            open={sections.isOpen("pinned")}
            onOpenChange={(open) => sections.setOpen("pinned", open)}
          >
            <NavList>
              {pinned.map((conversation) => (
                <ConversationRow
                  key={conversation.id}
                  conversation={conversation}
                  active={conversation.id === activeId}
                  actions={actions}
                />
              ))}
            </NavList>
          </NavSection>
        )}

        <NavSection
          title="Projects"
          open={sections.isOpen("projects")}
          onOpenChange={(open) => sections.setOpen("projects", open)}
          actions={
            <Tooltip>
              <TooltipTrigger asChild>
                <RowAction
                  aria-label="Add project"
                  className="size-icon-button-sm"
                  onClick={() => openAddProject()}
                >
                  <Plus />
                </RowAction>
              </TooltipTrigger>
              <TooltipContent side="bottom">Add project</TooltipContent>
            </Tooltip>
          }
        >
          <NavList>
            {projects.map((project) => (
              <ProjectRow
                key={project.id}
                project={project}
                sessions={sessions[project.id] ?? NO_SESSIONS}
                activeId={activeId}
                drafting={draftProjectId === project.id}
                actions={actions}
                onSettings={onProjectSettings}
                onRemove={setRemoving}
              />
            ))}
            {catalogLoaded && projects.length === 0 && (
              <li className="text-muted-foreground px-2 py-1 text-sm">
                Projects group sessions on a repository.{" "}
                <button
                  type="button"
                  className="text-foreground/85 underline-offset-4 hover:underline"
                  onClick={() => openAddProject()}
                >
                  Add a project
                </button>
              </li>
            )}
          </NavList>
        </NavSection>

        <NavSection
          title="Chats"
          open={sections.isOpen("chats")}
          onOpenChange={(open) => sections.setOpen("chats", open)}
          actions={
            <Tooltip>
              <TooltipTrigger asChild>
                <RowAction
                  aria-label="New chat"
                  className="size-icon-button-sm"
                  onClick={() => select({ type: "draft", kind: "chat" })}
                >
                  <Plus />
                </RowAction>
              </TooltipTrigger>
              <TooltipContent side="bottom">New chat</TooltipContent>
            </Tooltip>
          }
        >
          <NavList>
            {chats.map((conversation) => (
              <ConversationRow
                key={conversation.id}
                conversation={conversation}
                active={conversation.id === activeId}
                actions={actions}
              />
            ))}
            {catalogLoaded && chats.length === 0 && <EmptyState as="li" title="Chats you start appear here." />}
          </NavList>
        </NavSection>
      </div>

      {error && (
        <Banner tone="error" text={error} onDismiss={() => setError(null)} />
      )}

      <ProjectDialog
        open={dialog?.type === "projectSettings"}
        onOpenChange={(open) => !open && setDialog(null)}
        project={dialog?.type === "projectSettings" ? dialog.project : null}
      />
      <RemoveProjectDialog project={removing} onOpenChange={(open) => !open && setRemoving(null)} />
      <NameDialog
        open={dialog?.type === "rename"}
        onOpenChange={(open) => !open && setDialog(null)}
        title={
          dialog?.type === "rename" && dialog.conversation.kind === "chat"
            ? "Rename chat"
            : "Rename session"
        }
        label="Title"
        initialValue={dialog?.type === "rename" ? dialog.conversation.title : ""}
        confirmLabel="Rename"
        onSubmit={async (title) => {
          if (dialog?.type === "rename") {
            await renameConversation(dialog.conversation.id, title);
          }
        }}
      />
    </>
  );
}

const NO_SESSIONS: Conversation[] = [];

// ----- the strip -------------------------------------------------------------------------

/**
 * The Brigadier mark atop the collapsed strip, where it sits in the expanded header. Hovered
 * or focused it turns into the button that expands the sidebar. Resting the pointer on its row
 * peeks the sidebar beside the strip (the peek stands in for a tooltip); it stays lit meanwhile.
 */
export function StripMark() {
  const { setOpen, peek } = useSidebar();
  return (
    <div data-sidebar-peek-trigger className="h-nav-header flex shrink-0 items-center px-2">
      <TooltipIconButton
        look="strip"
        tooltip="Show sidebar"
        tip={false}
        data-state={peek === "open" ? "open" : undefined}
        className="group/mark"
        onClick={() => setOpen(true)}
      >
        <BrigadierGlyph className="size-5! group-hover/mark:hidden group-focus-visible/mark:hidden" />
        <Sidebar className="hidden group-hover/mark:block group-focus-visible/mark:block" />
      </TooltipIconButton>
    </div>
  );
}

/**
 * The Home panel collapsed: the mark, New chat, Search, and Projects and Chats, each opening
 * its list beside the strip.
 */
export function AppStrip() {
  const shortcuts = useShortcuts();
  const isChatDraft = useApp((s) => s.selection.type === "draft" && s.selection.kind === "chat");
  return (
    <>
      <StripMark />
      <NavList className="px-2 pt-1">
        <li>
          <TooltipIconButton look="strip" side="right" tooltip="New chat" pressed={isChatDraft} className="w-full" onClick={() => select({ type: "draft", kind: "chat" })}>
            <ComposeEditSquare />
          </TooltipIconButton>
        </li>
        <li>
          <TooltipIconButton look="strip" side="right" tooltip="Search" shortcut={shortcuts.search} className="w-full" onClick={() => openSearch()}>
            <MagnifyingGlassSearch />
          </TooltipIconButton>
        </li>
        <li>
          <Flyout label="Projects" icon={<Folders />}>
            {(pick) => <ProjectsFlyout pick={pick} />}
          </Flyout>
        </li>
        <li>
          <Flyout label="Chats" icon={<Chats />}>
            {(pick) => <ChatsFlyout pick={pick} />}
          </Flyout>
        </li>
      </NavList>
    </>
  );
}

/**
 * A strip icon that opens a list beside the strip, to pick from without expanding the sidebar.
 * It closes on a pick (`pick` runs the choice and closes it), on Esc and on a click outside.
 */
function Flyout({
  label,
  icon,
  children,
}: {
  label: string;
  icon: ReactNode;
  children: (pick: (choose: () => void) => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const pick = (choose: () => void) => {
    setOpen(false);
    choose();
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <TooltipIconButton look="strip" side="right" tooltip={label} className="w-full">{icon}</TooltipIconButton>
      </PopoverTrigger>
      <PopoverContent
        side="right"
        align="start"
        sideOffset={besideStrip()}
        aria-label={label}
        // Focus the list itself, not its first row, so Tab goes on into the rows.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus();
        }}
        className="flex max-h-(--radix-popover-content-available-height) w-72 flex-col overflow-y-auto p-1.5"
      >
        {children(pick)}
      </PopoverContent>
    </Popover>
  );
}

/** A chat or session in a flyout: its title and state; picking it opens it. */
function FlyoutConversation({
  conversation,
  active,
  nested = false,
  pick,
}: {
  conversation: Conversation;
  active: boolean;
  nested?: boolean;
  pick: (choose: () => void) => void;
}) {
  return (
    <li>
      <NavRow
        data-active={active}
        aria-current={active ? "page" : undefined}
        className={cn("pe-1.5", nested && "ps-8")}
        onClick={() => pick(() => openConversation(conversation.id))}
      >
        <SidebarError conversationId={conversation.id} />
        <span data-slot="row-title" className="mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap">
          {conversation.title}
        </span>
        <RowStatus conversationId={conversation.id} />
      </NavRow>
    </li>
  );
}

/** The Projects flyout: each project, picked to start a session in it, with its sessions. */
function ProjectsFlyout({ pick }: { pick: (choose: () => void) => void }) {
  const { projects, sessions } = useInboxSections();
  const activeId = useApp((s) => (s.selection.type === "conversation" ? s.selection.id : null));
  const draftProjectId = useApp((s) =>
    s.selection.type === "draft" && s.selection.kind === "session" ? s.selection.projectId : null,
  );
  if (projects.length === 0) {
    return (
      <p className="text-muted-foreground px-2 py-1 text-sm">
        Projects group sessions on a repository.{" "}
        <button
          type="button"
          className="text-foreground/85 underline-offset-4 hover:underline"
          onClick={() => pick(() => openAddProject())}
        >
          Add a project
        </button>
      </p>
    );
  }
  return (
    <NavList>
      {projects.map((project) => (
        <li key={project.id} className="flex flex-col">
          <NavRow
            title={`New session in ${project.name}`}
            data-active={draftProjectId === project.id}
            onClick={() =>
              pick(() => {
                setProjectExpanded(project.id, true);
                select({ type: "draft", kind: "session", projectId: project.id });
              })
            }
          >
            <Folder />
            <span className="mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap">
              {project.name}
            </span>
          </NavRow>
          <NavList className="pt-px pb-1">
            {(sessions[project.id] ?? NO_SESSIONS).map((conversation) => (
              <FlyoutConversation
                key={conversation.id}
                conversation={conversation}
                active={conversation.id === activeId}
                nested
                pick={pick}
              />
            ))}
          </NavList>
        </li>
      ))}
    </NavList>
  );
}

/** The Chats flyout: the pinned chats and sessions, then the chats. */
function ChatsFlyout({ pick }: { pick: (choose: () => void) => void }) {
  const { pinned, chats } = useSections();
  const activeId = useApp((s) => (s.selection.type === "conversation" ? s.selection.id : null));
  return (
    <div className="flex flex-col gap-3">
      {pinned.length > 0 && (
        <section>
          <DropdownMenuLabel variant="heading" asChild>
            <h2>Pinned</h2>
          </DropdownMenuLabel>
          <NavList>
            {pinned.map((conversation) => (
              <FlyoutConversation
                key={conversation.id}
                conversation={conversation}
                active={conversation.id === activeId}
                pick={pick}
              />
            ))}
          </NavList>
        </section>
      )}
      <section>
        <DropdownMenuLabel variant="heading" asChild>
          <h2>Chats</h2>
        </DropdownMenuLabel>
        <NavList>
          {chats.map((conversation) => (
            <FlyoutConversation
              key={conversation.id}
              conversation={conversation}
              active={conversation.id === activeId}
              pick={pick}
            />
          ))}
          {chats.length === 0 && <EmptyState as="li" title="Chats you start appear here." />}
        </NavList>
      </section>
    </div>
  );
}

const ProjectRow = memo(function ProjectRow({
  project,
  sessions,
  activeId,
  drafting,
  actions,
  onSettings,
  onRemove,
}: {
  project: Project;
  sessions: Conversation[];
  activeId: string | null;
  drafting: boolean;
  actions: RowActions;
  onSettings: (project: Project) => void;
  onRemove: (project: Project) => void;
}) {
  const expanded = useApp((s) => s.expandedProjects[project.id] ?? true);
  const mac = useApp((s) => s.info?.platform === "macos");
  const repo = project.repos[0]?.path;
  const newSession = () => {
    setProjectExpanded(project.id, true);
    select({ type: "draft", kind: "session", projectId: project.id });
  };
  return (
    <li className="flex flex-col">
      <div className="group/row relative">
        <NavRow
          aria-expanded={expanded}
          data-active={drafting}
          className="pe-16"
          onClick={() => setProjectExpanded(project.id, !expanded)}
        >
          {expanded ? <FolderOpen /> : <Folder />}
          <span className="mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap">
            {project.name}
          </span>
        </NavRow>
        <RowActionsSlot>
          <DropdownMenu modal={false}>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <RowAction aria-label={`Actions for ${project.name}`}>
                    <DotsHorizontal />
                  </RowAction>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="bottom">Project actions</TooltipContent>
            </Tooltip>
            <DropdownMenuContent side="bottom" align="start">
              <DropdownMenuItem onSelect={newSession}>
                <ComposeEditSquare />
                New session
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => onSettings(project)}>
                <SettingsCog />
                Project settings…
              </DropdownMenuItem>
              {repo && (
                <DropdownMenuItem
                  onSelect={() =>
                    void openFolder(repo).catch((error: unknown) => actions.onError(errorText(error)))
                  }
                >
                  <FolderOpen />
                  {mac ? "Reveal in Finder" : "Open in File Manager"}
                </DropdownMenuItem>
              )}
              {repo && (
                <DropdownMenuItem onSelect={() => void exportProjectConventions(project.id, repo)}>
                  <Download />
                  Export conventions to AGENTS.md…
                </DropdownMenuItem>
              )}
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={() => onRemove(project)}>
                <Trash />
                Remove project…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <RowAction
                aria-label={`New session in ${project.name}`}
                onClick={newSession}
              >
                <ComposeEditSquare />
              </RowAction>
            </TooltipTrigger>
            <TooltipContent side="bottom">New session</TooltipContent>
          </Tooltip>
        </RowActionsSlot>
      </div>
      <NavFold open={expanded}>
        <NavList className="pt-px pb-2">
          {sessions.map((conversation) => (
            <ConversationRow
              key={conversation.id}
              conversation={conversation}
              active={activeId === conversation.id}
              actions={actions}
              nested
            />
          ))}
          {sessions.length === 0 && <EmptyState as="li" className="ps-8" title="No sessions yet" />}
        </NavList>
      </NavFold>
    </li>
  );
});
