import { createContext, type CSSProperties, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { FilesTab } from "@/app/conversation/FilesTab";
import { ProjectDraftPane } from "@/app/conversation/ProjectDraftPane";
import { SearchPanel } from "@/app/conversation/SearchPanel";
import { SourcePanel } from "@/app/conversation/SourcePanel";
import { shortcutKeys } from "@/app/shortcutCatalog";
import { FilesGlyph, SearchGlyph, SidebarRightGlyph, SidebarRightOffGlyph, SourceControlGlyph } from "@/components/glyphs/codicons";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { SidebarResizeHandle, SidebarReveal, useSidebarWidth } from "@/components/ui/sidebar-layout";
import { cn } from "@/lib/utils";
import { browserFocus } from "@/ipc/client";
import { CHAT_TAB, openFileTab, useSessionTabs } from "@/state/sessionTabs";
import { isFilesSearchKey, openWorkspacePane, setWorkspacePaneOpen, useWorkspacePaneState, WORKSPACE_PANE_WIDTH_KEY, workspacePaneKey, type WorkspacePaneTab } from "@/state/workspacePane";
import { useApp } from "@/state/store";
import { prefetchWorkspace, useCheckoutKey, watchWorkspace, type WorkspaceTarget } from "@/state/workspaceCache";


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
    filePlaceholder: placeholder?.id === id ? placeholder.tabId : undefined, fileOpened, focusRequest,
  }), [size, enabled, open, active, toggleOpen, select, openFilesSearch, conversationId, projectId, searchRequest, searchHandled, placeholder, id, fileOpened, prefetch, focusRequest]);
}

export const WorkspacePaneContext = createContext<ReturnType<typeof useWorkspacePane> | null>(null);

/** The toolbar's right sidebar toggle: its glyph shows the sidebar out or in. */
export function WorkspacePaneToggle() {
  const pane = useContext(WorkspacePaneContext);
  const mac = useApp((s) => s.info?.platform === "macos");
  if (!pane?.enabled) return null;
  return (
    <TitlebarButton tooltip="Right sidebar" shortcut={shortcutKeys("right-sidebar", mac)}
      aria-pressed={pane.open} aria-controls="workspace-pane-content" onPointerEnter={pane.prefetch} onClick={pane.toggleOpen}>
      {pane.open ? <SidebarRightGlyph /> : <SidebarRightOffGlyph />}
    </TitlebarButton>
  );
}

/**
 * The right sidebar: Files or Source for the open session, or before it starts for its
 * project's folder. A full-height column at the window's edge, its views' icons in the titlebar
 * strip; the Workers panel, when open too, sits between it and the content.
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
        "column-divider relative flex h-full min-h-0 shrink-0 before:transition-opacity before:duration-300 before:ease-sidebar after:transition-opacity after:duration-300 after:ease-sidebar motion-reduce:before:transition-none motion-reduce:after:transition-none",
        !open && "before:opacity-0 after:opacity-0",
      )}
      style={{ "--right-sidebar-width": pane.width === null ? "var(--spacing-sidebar)" : `${pane.width}px` } as CSSProperties}>
      <SidebarReveal side="right" width="workspace-pane-width" open={open} hidden={!open} resizing={pane.resizing}>
        <div className="workspace-pane-width flex h-full min-h-0 flex-col">
          <div role="tablist" aria-label="Right sidebar" data-tauri-drag-region className="h-titlebar flex shrink-0 items-center justify-center gap-1 px-2">
            <TitlebarTips>
              {VIEWS.map(({ id, title, shortcut, Icon }, index) => (
                <TitlebarButton key={id} role="tab" id={`workspace-pane-${id}`} aria-controls="workspace-pane-content"
                  aria-selected={pane.active === id} tabIndex={pane.active === id ? 0 : -1}
                  className="aria-selected:bg-toolbar-pressed aria-selected:text-foreground"
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
                </TitlebarButton>
              ))}
            </TitlebarTips>
          </div>
          <div id="workspace-pane-content" role="tabpanel" aria-labelledby={`workspace-pane-${pane.active}`} data-pane={pane.active}
            className="bg-background text-sidebar-foreground rounded-e-page flex min-h-0 flex-1 flex-col overflow-hidden">
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
