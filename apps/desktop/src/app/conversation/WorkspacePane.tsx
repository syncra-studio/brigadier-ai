import { createContext, type CSSProperties, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { FilesTab } from "@/app/conversation/FilesTab";
import { ProjectDraftPane } from "@/app/conversation/ProjectDraftPane";
import { SearchPanel } from "@/app/conversation/SearchPanel";
import { SourcePanel } from "@/app/conversation/SourcePanel";
import { shortcutKeys } from "@/app/shortcutCatalog";
import { SidebarGlyph } from "@/app/SidebarToggle";
import { FilesGlyph, SearchGlyph, SourceControlGlyph } from "@/components/icons";
import { TitlebarTips } from "@/components/titlebar-button";
import { SidebarResizeHandle, SidebarReveal, useSidebarWidth } from "@/components/ui/sidebar-layout";
import { cn } from "@/lib/utils";
import { browserFocus } from "@/ipc/client";
import { CHAT_TAB, openFileTab, useSessionTabs } from "@/state/sessionTabs";
import { isFilesSearchKey, openWorkspacePane, setWorkspacePaneOpen, useWorkspacePaneState, WORKSPACE_PANE_WIDTH_KEY, workspacePaneKey, type WorkspacePaneTab } from "@/state/workspacePane";
import { useBoard, useCheckoutChanges } from "@/state/board";
import { useApp } from "@/state/store";
import { onWorkspaceChanged, prefetchWorkspace, readSource, useCachedEntry, useCheckoutKey, watchWorkspace, type WorkspaceTarget } from "@/state/workspaceCache";


/** The right sidebar's views, in its icon row's order. */
const VIEWS = [
  { id: "files", title: "Files", shortcut: "show-files", Icon: FilesGlyph },
  { id: "search", title: "Search", shortcut: "show-search", Icon: SearchGlyph },
  { id: "source", title: "Source", shortcut: "show-source", Icon: SourceControlGlyph },
] as const satisfies readonly { id: WorkspacePaneTab; title: string; shortcut: string; Icon: unknown }[];

export function useWorkspacePane(conversationId: string | null, projectId: string | null, enabled: boolean) {
  const id = conversationId ?? `draft:${projectId ?? "none"}`;
  const open = useWorkspacePaneState((s) => s.open);
  const active = useWorkspacePaneState((s) => s.tabs[id] ?? "files");
  const size = useSidebarWidth(WORKSPACE_PANE_WIDTH_KEY);
  const [searchRequest, setSearchRequest] = useState(0);
  const [placeholder, setPlaceholder] = useState<{ id: string; tabId: string } | null>(null);
  const sessionActive = useSessionTabs((s) => conversationId ? s.sessions[conversationId]?.active ?? CHAT_TAB : CHAT_TAB);
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
  // What the sidebar would show is read as the session or draft opens, so opening it paints at once.
  const prefetch = useCallback(() => {
    if (enabled && target) prefetchWorkspace(target);
  }, [enabled, target]);
  useEffect(prefetch, [prefetch, checkout]);
  // How many files Source lists, for its icon and the toggle's dot. While the sidebar is closed
  // nothing watches the checkout: what the board says changed it reads Source again.
  const source = useCachedEntry(checkout)?.source;
  const moved = useCheckoutChanges(conversationId ?? "");
  const edited = useBoard((s) => s.board && s.board.conversationId === conversationId
    ? Object.values(s.board.requests).map((entry) => `${entry.id}:${entry.state.type}:${entry.changes ? JSON.stringify(entry.changes.stat) : ""}`).join()
    : "");
  const closed = enabled && !open && target?.type === "conversation" ? target : null;
  const reread = useCallback(() => {
    if (closed) readSource(closed).catch(() => undefined);
  }, [closed]);
  useEffect(reread, [reread, moved, edited]);
  const changed = useMemo(() => source ? new Set([...source.staged, ...source.changes].map((file) => file.path)).size : 0, [source]);
  // While it shows, the daemon says when the checkout changes, so the views refresh in place.
  // A new connection forgets the old one's watch, so it is asked again after reconnecting.
  const connected = useApp((s) => s.connection.status === "connected");
  const watching = enabled && open && connected ? target : null;
  // It is asked again when the session moves to another checkout (its own copy made or gone).
  const watch = useCallback(() => {
    if (!watching) return;
    watchWorkspace(watching);
    return () => watchWorkspace(null);
  }, [watching]);
  useEffect(watch, [watch, checkout]);
  useEffect(() => {
    if (!watching) return;
    return onWorkspaceChanged(watching, () => { readSource(watching).catch(() => undefined); });
  }, [watching]);
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
 * The right sidebar: Files, Search or Source for the open session, or before it starts for its
 * project's folder. A column at the window's end edge below the titlebar, its views' icons in a
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
  // through the closing slide, then go, so a closed sidebar reads and refreshes nothing.
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
      ?.querySelector<HTMLElement>('input, textarea, button, [tabindex="0"]')?.focus({ preventScroll: true });
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
          <div role="tablist" aria-label="Right sidebar" className="h-page-toolbar bg-background flex shrink-0 items-center justify-center gap-1 px-2">
            <TitlebarTips>
              {VIEWS.map(({ id, title, shortcut, Icon }, index) => (
                <TooltipIconButton look="toolbar" key={id} role="tab" id={`workspace-pane-${id}`} aria-controls="workspace-pane-content"
                  aria-selected={pane.active === id} tabIndex={pane.active === id ? 0 : -1}
                  className="aria-selected:bg-toolbar-pressed aria-selected:text-foreground relative"
                  tooltip={title} shortcut={shortcutKeys(shortcut, mac)}
                  onClick={() => pane.select(id)}
                  onKeyDown={(event) => {
                    const next = event.key === "ArrowRight" ? (index + 1) % VIEWS.length
                      : event.key === "ArrowLeft" ? (index + VIEWS.length - 1) % VIEWS.length
                        : event.key === "Home" ? 0 : event.key === "End" ? VIEWS.length - 1 : null;
                    if (next === null) return;
                    event.preventDefault();
                    const view = VIEWS[next]!;
                    pane.select(view.id);
                    document.getElementById(`workspace-pane-${view.id}`)?.focus();
                  }}>
                  <Icon />
                  {id === "source" && pane.changed > 0 && (
                    <span aria-hidden data-slot="source-count"
                      className="bg-link text-background absolute -end-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-2xs leading-none tabular-nums">
                      {pane.changed > 99 ? "99+" : pane.changed}
                    </span>
                  )}
                </TooltipIconButton>
              ))}
            </TitlebarTips>
          </div>
          <div id="workspace-pane-content" role="tabpanel" aria-labelledby={`workspace-pane-${pane.active}`} data-pane={pane.active}
            className="bg-background text-sidebar-foreground rounded-ee-page flex min-h-0 flex-1 flex-col overflow-clip">
            {/* Keyed by whose files they are: one draft's list or message never shows for another. */}
            {shown && (pane.conversationId ? (
              pane.active === "files" ? <FilesTab key={pane.conversationId} conversationId={pane.conversationId} searchRequest={pane.searchRequest}
                onSearchHandled={pane.searchHandled} replaceTabId={pane.filePlaceholder} onFileOpened={pane.fileOpened} />
                : pane.active === "search" ? <SessionSearch key={pane.conversationId} conversationId={pane.conversationId} />
                  : <SourcePanel key={pane.conversationId} target={{ type: "conversation", conversationId: pane.conversationId }} />
            ) : pane.projectId && <ProjectDraftPane key={`project:${pane.projectId}`} projectId={pane.projectId} tab={pane.active}
              searchRequest={pane.searchRequest} onSearchHandled={pane.searchHandled} />)}
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
