import { createContext, type CSSProperties, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { gitCommandsKey } from "@/app/conversation/cliFacts";
import { TooltipIconButton } from "@/components/controls";
import { PaneViewTabs } from "@/app/side-panel/PaneViewTabs";
import { FilesTab } from "@/app/side-panel/FilesTab";
import { ProjectDraftPane } from "@/app/side-panel/ProjectDraftPane";
import { SearchPanel } from "@/app/side-panel/SearchPanel";
import { SessionSource } from "@/app/side-panel/WorkerSource";
import { shortcutKeys } from "@/app/shortcutCatalog";
import { SidebarGlyph } from "@/components/shared";
import { SidebarResizeHandle, SidebarReveal, useSidebarWidth } from "@/components/layout";
import { cn } from "@/lib/utils";
import { browserFocus } from "@/ipc/client";
import { CHAT_TAB, openFileTab, useSessionTabs } from "@/state/sessionTabs";
import { isFilesSearchKey, openWorkspacePane, setWorkspacePaneOpen, useWorkspacePaneState, WORKSPACE_PANE_WIDTH_KEY, workspacePaneKey, type WorkspacePaneTab } from "@/state/workspacePane";
import { useCheckoutChanges } from "@/state/board";
import { useCliThread, useThreadFact } from "@/state/cliThread";
import { useApp } from "@/state/store";
import { onWorkspaceChanged, prefetchWorkspace, readSource, readUserWorktrees, refreshSource, uncommittedFiles, useCachedEntry, useCheckoutKey, useUserWorktrees, watchTargets, watchWorkspaces, type WorkspaceTarget } from "@/state/workspaceCache";


export function useWorkspacePane(conversationId: string | null, projectId: string | null, enabled: boolean) {
  const id = conversationId ?? `draft:${projectId ?? "none"}`;
  const open = useWorkspacePaneState((s) => s.open);
  const active = useWorkspacePaneState((s) => s.tabs[id] ?? "files");
  const size = useSidebarWidth(WORKSPACE_PANE_WIDTH_KEY);
  const [searchRequest, setSearchRequest] = useState(0);
  const [placeholder, setPlaceholder] = useState<{ id: string; tabId: string } | null>(null);
  const sessionActive = useSessionTabs((s) => s.sessions[id]?.active ?? CHAT_TAB);
  if (placeholder && (!open || active !== "files" || sessionActive !== placeholder.tabId || id !== placeholder.id)) setPlaceholder(null);
  const fileOpened = useCallback(() => setPlaceholder(null), []);
  const searchHandled = useCallback(() => setSearchRequest(0), []);
  const toggleOpen = useCallback(() => {
    if (enabled) setWorkspacePaneOpen((value) => !value);
  }, [enabled]);
  const select = useCallback((tab: WorkspacePaneTab) => {
    if (enabled) openWorkspacePane(id, tab);
  }, [enabled, id]);
  const openFilesSearch = useCallback((replaceTabId?: string) => {
    if (!enabled) return;
    openWorkspacePane(id, "files");
    setSearchRequest((value) => value + 1);
    setPlaceholder(replaceTabId ? { id, tabId: replaceTabId } : null);
  }, [enabled, id]);
  const target = useMemo<WorkspaceTarget | null>(() => conversationId ? { type: "conversation", conversationId }
    : projectId ? { type: "project", projectId } : null, [conversationId, projectId]);
  const checkout = useCheckoutKey(target ?? { type: "project", projectId: "" });
  const connected = useApp((s) => s.connection.status === "connected");
  // What the sidebar would show is read as the session or draft opens, so opening it paints at once.
  const prefetch = useCallback(() => {
    if (enabled && target) prefetchWorkspace(target);
  }, [enabled, target]);
  useEffect(prefetch, [prefetch, checkout, connected]);
  // How many files Source lists, for its icon, the toggle's dot and the summary card. They follow
  // the checkout whichever view shows, and with the sidebar closed.
  const source = useCachedEntry(checkout)?.source;
  const landed = useCheckoutChanges(conversationId ?? "");
  // A CLI turn that starts or ends, or a git command of its thread, may have changed it.
  const edited = useCliThread((s) => {
    const turns = conversationId ? s.views[conversationId]?.turns : undefined;
    return turns ? `${turns.length}:${turns.at(-1)?.end ?? ""}` : "";
  });
  const git = useThreadFact(conversationId, gitCommandsKey);
  const moves = `${landed}\n${edited}\n${git}`;
  const reading = enabled ? target : null;
  // Opening a checkout is the prefetch's read: only a move in the one already open reads here.
  const lastMoves = useRef({ checkout, moves });
  useEffect(() => {
    const movedHere = lastMoves.current.checkout === checkout && lastMoves.current.moves !== moves;
    lastMoves.current = { checkout, moves };
    if (movedHere && reading) refreshSource(reading).catch(() => undefined);
  }, [checkout, moves, reading]);
  useEffect(() => {
    if (!reading) return;
    const read = () => { readSource(reading).catch(() => undefined); };
    // What changed while the user was in another app, and what the watch or a view says changed.
    window.addEventListener("focus", read);
    const unsubscribe = onWorkspaceChanged(reading, read);
    return () => {
      window.removeEventListener("focus", read);
      unsubscribe();
    };
  }, [reading]);
  const changed = useMemo(() => uncommittedFiles(source), [source]);
  // While it shows, the daemon says when the checkout changes, so the views refresh in place.
  // A new connection forgets the old one's watch, so it is asked again after reconnecting.
  // A draft's Source has a row for each worktree the user made: read as it opens, after a
  // reconnect, on focus, and when the project's folder says one came or went.
  const draftProject = enabled && connected && !conversationId ? projectId : null;
  const worktrees = useUserWorktrees(draftProject);
  useEffect(() => {
    if (!draftProject) return;
    const read = () => { readUserWorktrees(draftProject).catch(() => undefined); };
    read();
    window.addEventListener("focus", read);
    const unsubscribe = onWorkspaceChanged({ type: "project", projectId: draftProject }, read);
    return () => {
      window.removeEventListener("focus", read);
      unsubscribe();
    };
  }, [draftProject]);
  const watching = enabled && open && connected ? target : null;
  // The whole set, asked again when it changes: a row came or went, the view changed, a reconnect
  // (`connected` above), or the session moved to another checkout (its own copy made or gone).
  const watched = JSON.stringify(watchTargets(watching, worktrees, active === "source"));
  useEffect(() => {
    const targets = JSON.parse(watched) as WorkspaceTarget[];
    if (targets.length === 0) return;
    watchWorkspaces(targets);
    return () => watchWorkspaces([]);
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [watched, checkout]);
  const mac = useApp((s) => s.info?.platform === "macos");
  // A native browser page has its own first responder. DOM focus alone cannot take the
  // keyboard back from it when a menu key opens the sidebar's filter or focuses a view.
  const withKeyboard = useCallback((act: () => void) => {
    if (mac && !document.hasFocus()) void browserFocus(null).then(act).catch(act);
    else act();
  }, [mac]);
  // A view's key opens the sidebar on it; pressed again while it shows, it takes the keyboard
  // there. Search's takes it at once, in its field.
  const [focusRequest, setFocusRequest] = useState(0);
  const show = useCallback((tab: WorkspacePaneTab) => {
    const shown = open && active === tab;
    if (!shown) select(tab);
    if (shown || tab === "search") setFocusRequest((count) => count + 1);
  }, [open, active, select]);
  useEffect(() => {
    if (!enabled) return;
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (isFilesSearchKey(event, mac)) {
        event.preventDefault();
        withKeyboard(openFilesSearch);
        return;
      }
      const asked = workspacePaneKey(event, mac);
      if (!asked) return;
      event.preventDefault();
      if (asked === "toggle") toggleOpen();
      else withKeyboard(() => show(asked));
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [enabled, mac, openFilesSearch, toggleOpen, show, withKeyboard]);
  return useMemo(() => ({
    ...size, enabled, open: enabled && open, active, toggleOpen, select, openFilesSearch, prefetch,
    setOpen: setWorkspacePaneOpen, conversationId, projectId, searchRequest, searchHandled,
    filePlaceholder: placeholder?.id === id ? placeholder.tabId : undefined, fileOpened, focusRequest, changed,
  }), [size, enabled, open, active, toggleOpen, select, openFilesSearch, conversationId, projectId, searchRequest, searchHandled, placeholder, id, fileOpened, prefetch, focusRequest, changed]);
}

export const WorkspacePaneContext = createContext<ReturnType<typeof useWorkspacePane> | null>(null);

/**
 * The toolbar's right sidebar toggle: the left sidebar toggle's icon, mirrored. While the session
 * has changes Source lists and Source isn't in view, a dot on it points there (Q91).
 */
export function WorkspacePaneToggle() {
  const pane = useContext(WorkspacePaneContext);
  const mac = useApp((s) => s.info?.platform === "macos");
  if (!pane?.enabled) return null;
  const changes = pane.changed > 0 && !(pane.open && pane.active === "source");
  return (
    <TooltipIconButton look="toolbar" tooltip={changes ? `Right sidebar · ${pane.changed === 1 ? "1 change" : `${pane.changed} changes`} in Source` : "Right sidebar"}
      shortcut={shortcutKeys("right-sidebar", mac)} aria-label="Right sidebar" className="relative"
      aria-pressed={pane.open} aria-controls="workspace-pane-content" onPointerEnter={pane.prefetch} onClick={pane.toggleOpen}>
      <SidebarGlyph side="right" open={pane.open} />
      {changes && <span data-slot="source-changes-dot" aria-label="Changes in Source" className="bg-link absolute end-1 top-1 size-2 rounded-full" />}
    </TooltipIconButton>
  );
}

/**
 * The right sidebar: Files, Search or Source (the session's checkout, or a worker's changes) for
 * the open session, or before it starts for its project's folder. A column at the window's end edge below the titlebar, its views' icons in a
 * row at its top; the Workers panel, when open too, sits between it and the content.
 */
export function WorkspacePane() {
  const pane = useContext(WorkspacePaneContext);
  const mac = useApp((s) => s.info?.platform === "macos");
  const open = Boolean(pane?.open);
  // While it shows, the sidebar and the Workers panel leave it at least its minimum width.
  useLayoutEffect(() => {
    const wrapper = open ? document.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]') : null;
    wrapper?.style.setProperty("--workspace-pane-reserve", "var(--spacing-sidebar-min)");
    return () => {
      wrapper?.style.removeProperty("--workspace-pane-reserve");
    };
  }, [open]);
  // It slides open and closed as the sidebar does, pushing the content aside. Its contents stay
  // through the closing slide, then go, so a closed sidebar's views read and refresh nothing.
  const column = useRef<HTMLElement>(null);
  const [shown, setShown] = useState(open);
  if (open && !shown) setShown(true);
  useEffect(() => {
    if (open || !shown) return;
    const slide = column.current?.firstElementChild;
    const style = slide ? getComputedStyle(slide) : null;
    // Reduced motion has no slide, so the contents go at once.
    const ms = style && style.transitionProperty !== "none"
      ? (Number.parseFloat(style.transitionDuration) + Number.parseFloat(style.transitionDelay)) * 1000 : 0;
    const timer = setTimeout(() => setShown(false), ms);
    return () => clearTimeout(timer);
  }, [open, shown]);
  // Once the view a key asked for shows, its first control takes the keyboard.
  const focusRequest = pane?.focusRequest ?? 0;
  useEffect(() => {
    if (!focusRequest) return;
    document.getElementById("workspace-pane-content")
      ?.querySelector<HTMLElement>('input:not(:disabled), textarea:not(:disabled), button:not(:disabled), [tabindex="0"]')?.focus({ preventScroll: true });
  }, [focusRequest]);
  if (!pane?.enabled) return null;
  return (
    <aside ref={column} aria-label="Right sidebar" data-slot="workspace-pane" data-state={open ? "expanded" : "collapsed"}
      className={cn(
        "start-hairline relative flex h-full min-h-0 shrink-0 before:transition-opacity before:duration-300 before:ease-sidebar motion-reduce:before:transition-none",
        !open && "before:opacity-0",
      )}
      style={{ "--right-sidebar-width": pane.width === null ? "var(--spacing-sidebar)" : `${pane.width}px` } as CSSProperties}>
      <SidebarReveal side="right" width="workspace-pane-width" open={open} hidden={!open} resizing={pane.resizing}>
        <div className="workspace-pane-width flex h-full min-h-0 flex-col">
          <PaneViewTabs active={pane.active} changed={pane.changed} mac={mac} onSelect={pane.select} />
          <div id="workspace-pane-content" role="tabpanel" aria-labelledby={`workspace-pane-${pane.active}`} data-pane={pane.active}
            className="bg-background text-sidebar-foreground rounded-ee-page flex min-h-0 flex-1 flex-col overflow-clip">
            {/* Keyed by whose files they are: one draft's list or message never shows for another. */}
            {shown && (pane.conversationId ? (
              pane.active === "files" ? <FilesTab key={pane.conversationId} conversationId={pane.conversationId} searchRequest={pane.searchRequest}
                onSearchHandled={pane.searchHandled} replaceTabId={pane.filePlaceholder} onFileOpened={pane.fileOpened} />
                : pane.active === "search" ? <SessionSearch key={pane.conversationId} conversationId={pane.conversationId} />
                  : <SessionSource key={pane.conversationId} conversationId={pane.conversationId} />
            ) : pane.projectId && <ProjectDraftPane key={`project:${pane.projectId}`} projectId={pane.projectId} tab={pane.active}
              searchRequest={pane.searchRequest} onSearchHandled={pane.searchHandled} replaceTabId={pane.filePlaceholder} onFileOpened={pane.fileOpened} />)}
          </div>
        </div>
      </SidebarReveal>
      {open && <SidebarResizeHandle {...pane} side="right" label="Resize right sidebar" />}
    </aside>
  );
}

/** A session's Search: a result opens its file at the line; no Replace before its own copy exists. */
function SessionSearch({ conversationId }: { conversationId: string }) {
  const target = useMemo(() => ({ type: "conversation", conversationId }) as const, [conversationId]);
  const waiting = useApp((s) => {
    const setup = s.conversations[conversationId]?.setup;
    return setup?.type === "session" && setup.environment.type === "newWorktree" && !setup.environment.path;
  });
  const open = useCallback((path: string, line: number, keep: boolean) => openFileTab(conversationId, path, { line, preview: !keep }), [conversationId]);
  return <SearchPanel target={target} onOpen={open} readOnly={waiting} />;
}
