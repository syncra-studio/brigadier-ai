import {
  Archive,
  ComposeEditSquare,
  DotsHorizontal,
  Download,
  Folder,
  FolderOpen,
  MagnifyingGlassSearch,
  Pencil,
  Pin,
  Plus,
  Settings,
  SettingsCog,
  Trash,
  Unpin,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { memo, useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";

import { errorText } from "@/app/dialogs/fields";
import { ProjectDialog } from "@/app/dialogs/ProjectDialog";
import { RemoveProjectDialog } from "@/app/dialogs/RemoveProjectDialog";
import { NameDialog } from "@/app/NameDialog";
import { useShortcuts } from "@/app/shortcuts";
import { KeepAwakeMenu, UsageMenu } from "@/app/RailStatus";
import { openSearch } from "@/app/SearchDialog";
import {
  navRow,
  NavEmpty,
  NavFold,
  NavHeader,
  NavList,
  NavSection,
  RailButton,
  rowAction,
} from "@/app/sidebar/nav";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { Spinner } from "@/components/glyphs/spinner";
import { TITLEBAR_BUTTON, TitlebarTips } from "@/components/titlebar-button";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Kbd } from "@/components/ui/kbd";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { openFolder } from "@/ipc/client";
import type { Conversation, Project } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  archive,
  archiveAll,
  closeSettings,
  openConversation,
  openSettings,
  renameConversation,
  select,
  setPinned,
  setProjectExpanded,
} from "@/state/actions";
import { useRowActivity } from "@/state/activity";
import { openAddProject } from "@/state/addProject";
import { exportProjectConventions } from "@/state/brain";
import { askDelete, clearPicked, pickClick, pickedIn, prunePicked, usePicked } from "@/state/picking";
import { useApp } from "@/state/store";

// ----- titlebar and rail -----------------------------------------------------------------

/**
 * The sidebar toggle, at a fixed spot in the titlebar on the traffic lights' line: just after
 * them on macOS, at the titlebar's start in full screen (where they are gone) and elsewhere.
 * It stays put while the panel opens and closes.
 */
export function TitlebarToggle() {
  const { open } = useSidebar();
  const { sidebar } = useShortcuts();
  return (
    <div className="h-titlebar-toggle start-titlebar-start absolute top-0 z-20 flex items-center">
      <TitlebarTips>
        <Tooltip>
          <TooltipTrigger asChild>
            <SidebarTrigger className={TITLEBAR_BUTTON} />
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {open ? "Hide sidebar" : "Show sidebar"}
            <Kbd>{sidebar}</Kbd>
          </TooltipContent>
        </Tooltip>
      </TitlebarTips>
    </div>
  );
}

/**
 * The navigation rail along the window's start, on the window chrome: Home (chats and
 * projects, under Brigadier's mark) at the top; at the bottom keeping the computer awake, the
 * agents' usage, and Settings (where the Inspector and setup are too).
 */
export function AppRail() {
  const { open, holdPeek, releasePeek } = useSidebar();
  const inSettings = useApp((s) => s.selection.type === "settings");
  const onUsagePage = useApp(
    (s) => s.selection.type === "settings" && s.selection.page === "usage",
  );
  const shortcuts = useShortcuts();
  return (
    <nav
      aria-label="App navigation"
      className="w-rail pt-titlebar flex h-full shrink-0 flex-col items-center gap-2 px-2 pb-1"
    >
      <div data-tauri-drag-region className="flex w-full flex-col items-center gap-2 pt-2">
        <RailButton
          label="Home"
          selected={!inSettings}
          onClick={() => closeSettings()}
          // With the panel closed, resting on it peeks the panel.
          onPointerEnter={open ? undefined : holdPeek}
          onPointerLeave={open ? undefined : releasePeek}
        >
          {/* The mark stands a size above the rail's icons, and bright whether or not Home is
              the page. */}
          <BrigadierGlyph aria-hidden className="text-foreground size-rail-mark!" />
        </RailButton>
      </div>
      <div data-tauri-drag-region className="min-h-0 w-full flex-1" />
      <KeepAwakeMenu />
      <UsageMenu />
      <RailButton
        label="Settings"
        shortcut={shortcuts.settings}
        selected={inSettings && !onUsagePage}
        onClick={() => openSettings()}
      >
        <Settings />
      </RailButton>
    </nav>
  );
}

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

/** What the rows can ask the sidebar to do. */
type RowActions = {
  onRename: (conversation: Conversation) => void;
  onError: (error: string) => void;
  /** A click with ⌘ (Ctrl off macOS) or Shift picks rows; false for a plain click. */
  onPick: (id: string, event: MouseEvent) => boolean;
  /** The picked rows, in the order the sidebar shows them. */
  picked: () => string[];
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
  const { pinned, chats, projects, sessions } = useSections();
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
        title="Brigadier"
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
          <button
            type="button"
            data-active={isChatDraft}
            className={navRow}
            onClick={() => select({ type: "draft", kind: "chat" })}
          >
            <ComposeEditSquare />
            <span className="truncate">New chat</span>
          </button>
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
                <button
                  type="button"
                  aria-label="Add project"
                  className={cn(rowAction, "size-icon-button-sm")}
                  onClick={() => openAddProject()}
                >
                  <Plus />
                </button>
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
            {catalogLoaded && chats.length === 0 && <NavEmpty>Chats you start appear here.</NavEmpty>}
          </NavList>
        </NavSection>
      </div>

      {error && (
        <div
          role="alert"
          className="border-destructive/40 bg-destructive/10 text-destructive rounded-control m-2 flex items-start gap-2 border px-2 py-1.5 text-xs"
        >
          <p className="min-w-0 flex-1 wrap-break-word">{error}</p>
          <Button variant="ghost" size="icon-xs" aria-label="Dismiss" onClick={() => setError(null)}>
            <X />
          </Button>
        </div>
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

/** A row's hover actions, over its end; the row's status gives way to them. */
function RowActionsSlot({ children }: { children: ReactNode }) {
  return (
    <div className="pointer-events-none absolute inset-y-0 end-1.5 flex items-center gap-2 opacity-0 group-focus-within/row:pointer-events-auto group-focus-within/row:opacity-100 group-hover/row:pointer-events-auto group-hover/row:opacity-100 has-data-[state=open]:pointer-events-auto has-data-[state=open]:opacity-100">
      {children}
    </div>
  );
}

/** Keeps a row's title clear of its actions while they show. */
const roomForActions =
  "group-hover/row:pe-14 group-focus-within/row:pe-14 group-has-data-[state=open]/row:pe-14";

/** Hides a row's trailing status while its actions show. */
const hideOnRowHover =
  "group-hover/row:invisible group-focus-within/row:invisible group-has-data-[state=open]/row:invisible";

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
        <button
          type="button"
          aria-expanded={expanded}
          data-active={drafting}
          className={cn(navRow, "pe-16")}
          onClick={() => setProjectExpanded(project.id, !expanded)}
        >
          {expanded ? <FolderOpen /> : <Folder />}
          <span className="mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap">
            {project.name}
          </span>
        </button>
        <RowActionsSlot>
          <DropdownMenu modal={false}>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <button type="button" aria-label={`Actions for ${project.name}`} className={rowAction}>
                    <DotsHorizontal />
                  </button>
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
              <button
                type="button"
                aria-label={`New session in ${project.name}`}
                className={rowAction}
                onClick={newSession}
              >
                <ComposeEditSquare />
              </button>
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
              active={conversation.id === activeId}
              actions={actions}
              nested
            />
          ))}
          {sessions.length === 0 && <NavEmpty className="ps-8">No sessions yet</NavEmpty>}
        </NavList>
      </NavFold>
    </li>
  );
});

/**
 * At the end of a row, showing a chat's state: a green "Awaiting approval" or "Needs input"
 * pill while it waits for the user, else a spinner while it runs. The row's actions take its
 * place on hover.
 */
function RowStatus({ conversationId }: { conversationId: string }) {
  const { running, awaiting } = useRowActivity(conversationId);
  if (!running && !awaiting) return null;
  if (awaiting) {
    return (
      <span
        data-slot="row-status"
        data-status={awaiting}
        className={cn(
          "bg-success/15 text-success rounded-capsule h-pill px-pill flex shrink-0 items-center text-2xs whitespace-nowrap",
          hideOnRowHover,
        )}
      >
        {awaiting === "approval" ? "Awaiting approval" : "Needs input"}
      </span>
    );
  }
  return (
    <Spinner
      data-slot="row-status"
      data-status="running"
      aria-label="Running"
      className={cn(
        "text-muted-foreground size-icon-sm shrink-0 animate-spin motion-reduce:animate-none",
        hideOnRowHover,
      )}
    />
  );
}

/**
 * A chat or session: its title, its state at the end, and on hover Pin and Archive. A
 * right-click opens its menu (Pin, Rename…, Archive, Delete…), or on one of several picked
 * rows the menu for all of them (Archive (N), Delete (N)…). ⌘-click (Ctrl-click off macOS)
 * picks it, Shift-click picks a range.
 */
const ConversationRow = memo(function ConversationRow({
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
  const pinned = conversation.pinnedAtMs !== null;
  const noun = conversation.kind === "chat" ? "chat" : "session";
  const togglePin = () =>
    void setPinned(conversation.id, !pinned).catch((error: unknown) =>
      actions.onError(errorText(error)),
    );
  const archiveIt = () =>
    void archive(conversation.id).catch((error: unknown) => actions.onError(errorText(error)));
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
          <button
            type="button"
            data-active={active}
            data-selected={picked}
            aria-current={active ? "page" : undefined}
            className={cn(
              navRow,
              "data-[selected=true]:bg-link/20 data-[selected=true]:text-foreground pe-1.5",
              roomForActions,
              nested && "ps-8",
            )}
            onClick={(event) => {
              if (!actions.onPick(conversation.id, event)) openConversation(conversation.id);
            }}
          >
            <span className="mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap">
              {conversation.title}
            </span>
            <RowStatus conversationId={conversation.id} />
          </button>
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
            <button
              type="button"
              aria-label={`${pinned ? "Unpin" : "Pin"} ${conversation.title}`}
              className={rowAction}
              onClick={togglePin}
            >
              {pinned ? <Unpin /> : <Pin />}
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">{pinned ? `Unpin ${noun}` : `Pin ${noun}`}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-label={`Archive ${conversation.title}`}
              className={rowAction}
              onClick={archiveIt}
            >
              <Archive />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Archive {noun}</TooltipContent>
        </Tooltip>
      </RowActionsSlot>
    </li>
  );
});
