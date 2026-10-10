import { Agent, Branch, Folders, SidebarFloatingRight, SidebarRight } from "@openai/apps-sdk-ui/components/Icon";
import { type CSSProperties, createContext, lazy, Suspense, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { WorkersTab } from "@/app/conversation/Agents";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { SidebarReveal, SidebarResizeHandle, useSidebarChoice, useSidebarWidth } from "@/components/ui/sidebar-layout";
import { useSidebar } from "@/components/ui/sidebar";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { isRightSidebarKey, rightSidebarFolds, rightSidebarToggleLabel, type RightSidebarTab, selectRightSidebarTab, setRightSidebarOpen, useRightSidebarState } from "@/state/rightSidebar";
import { useApp } from "@/state/store";

const FilesTab = lazy(() => import("@/app/conversation/FilesTab").then((m) => ({ default: m.FilesTab })));
const SourcePanel = lazy(() => import("@/app/conversation/SourcePanel").then((m) => ({ default: m.SourcePanel })));
const TABS = [
  { id: "files", title: "Files", Icon: Folders },
  { id: "source", title: "Source", Icon: Branch },
  { id: "workers", title: "Workers", Icon: Agent },
] as const;

export function useRightSidebar(conversationId: string | null, enabled: boolean) {
  const left = useSidebar();
  const [fold, setFold] = useState(() => window.innerWidth < tokenPx("--spacing-narrow-window"));
  useLayoutEffect(() => {
    if (!enabled) return;
    const panel = document.querySelector<HTMLElement>('[data-slot="sidebar-panel"]');
    // Only a fold transition updates React; resizing and animation frames stay in CSS.
    const measure = () => setFold(rightSidebarFolds(
      window.innerWidth, left.open, panel?.getBoundingClientRect().width ?? 0,
      tokenPx("--spacing-narrow-window"),
    ));
    measure();
    const observer = new ResizeObserver(measure);
    if (panel) observer.observe(panel);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [enabled, left.open]);
  const wideOpen = useRightSidebarState((s) => s.open);
  const active = useRightSidebarState((s) => conversationId ? s.tabs[conversationId] ?? "files" : "files");
  const { open, setOpen, toggleSidebar } = useSidebarChoice(wideOpen, setRightSidebarOpen, false, fold);
  const { width, setWidth, resizing, setResizing } = useSidebarWidth("brigadier.rightSidebarWidth");
  const [searchRequest, setSearchRequest] = useState(0);
  const searchHandled = useCallback(() => setSearchRequest(0), []);
  const mac = useApp((s) => s.info?.platform === "macos");
  const selectTab = useCallback((tab: RightSidebarTab) => {
    if (enabled && conversationId) selectRightSidebarTab(conversationId, tab);
  }, [conversationId, enabled]);
  const openTab = useCallback((tab: RightSidebarTab, search = false) => {
    if (!enabled) return;
    selectTab(tab);
    setOpen(true);
    if (search) setSearchRequest((value) => value + 1);
  }, [enabled, selectTab, setOpen]);
  useEffect(() => {
    if (!enabled) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !isRightSidebarKey(event, mac)) return;
      event.preventDefault();
      if (!event.repeat) toggleSidebar();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled, mac, toggleSidebar]);
  return useMemo(() => ({
    width, setWidth, resizing, setResizing, enabled, open: enabled && open, active,
    setOpen, toggleSidebar, selectTab, openTab, searchRequest, searchHandled,
  }), [width, setWidth, resizing, setResizing, enabled, open, active, setOpen,
    toggleSidebar, selectTab, openTab, searchRequest, searchHandled]);
}

export const RightSidebarContext = createContext<ReturnType<typeof useRightSidebar> | null>(null);

export function RightSidebarToggle() {
  const rightSidebar = useContext(RightSidebarContext);
  const mac = useApp((s) => s.info?.platform === "macos");
  if (!rightSidebar?.enabled) return null;
  return (
    <>
      {!rightSidebar.open && <span aria-hidden className="w-icon-button-md shrink-0" />}
      {/* Fixed to the window edge, just like the left titlebar controls, throughout the spring. */}
      <div className="h-titlebar end-surface-inset fixed top-0 z-30 flex items-center pe-1">
        <TitlebarTips>
          <TitlebarButton
            data-slot="right-sidebar-toggle"
            tooltip={rightSidebarToggleLabel(rightSidebar.open)}
            shortcut={mac ? "⌥⌘B" : "Ctrl+Alt+B"}
            aria-expanded={rightSidebar.open}
            onClick={rightSidebar.toggleSidebar}
          >
            {rightSidebar.open ? <SidebarRight /> : <SidebarFloatingRight />}
          </TitlebarButton>
        </TitlebarTips>
      </div>
    </>
  );
}

/** The full-height right column. Closing reveals no strip and installs no hover trigger. */
export function RightSidebar({ conversationId }: { conversationId: string }) {
  const panel = useContext(RightSidebarContext);
  const content = useRef<HTMLDivElement>(null);
  const column = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = column.current;
    const left = element?.closest<HTMLElement>('[data-slot="sidebar-wrapper"]');
    if (!element || !left) return;
    const measure = () => left.style.setProperty("--right-sidebar-shown", `${element.getBoundingClientRect().width}px`);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
      left.style.removeProperty("--right-sidebar-shown");
    };
  }, []);
  const [mounted, setMounted] = useState(panel?.open ?? false);
  if (panel?.open && !mounted) setMounted(true);
  useEffect(() => {
    if (!panel?.open && content.current?.contains(document.activeElement)) {
      Array.from(document.querySelectorAll<HTMLElement>('[data-slot="right-sidebar-toggle"]'))
        .find((button) => button.getClientRects().length > 0)?.focus({ preventScroll: true });
    }
  }, [panel?.open]);
  if (!panel?.enabled) return null;
  return (
    <div
      ref={column}
      data-slot="right-sidebar"
      data-state={panel.open ? "expanded" : "collapsed"}
      className={cn("relative flex h-full shrink-0", panel.open && "column-divider")}
      style={{ "--right-sidebar-width": panel.width === null ? "var(--spacing-sidebar)" : `${panel.width}px` } as CSSProperties}
    >
      <SidebarReveal side="right" open={panel.open} hidden={!panel.open} resizing={panel.resizing}>
        <div ref={content} className="right-sidebar-panel-width flex h-full flex-col">
          <div role="tablist" aria-label="Right sidebar" data-tauri-drag-region className="h-titlebar flex shrink-0 items-center gap-1 ps-2 pe-icon-button-lg">
            <TitlebarTips>
              {TABS.map(({ id, title, Icon }, index) => (
                <TitlebarButton
                  key={id}
                  role="tab"
                  id={`right-sidebar-${id}`}
                  aria-controls="right-sidebar-content"
                  aria-selected={panel.active === id}
                  className="aria-selected:bg-toolbar-pressed aria-selected:text-foreground"
                  tabIndex={panel.active === id ? 0 : -1}
                  tooltip={title}
                  onClick={() => panel.selectTab(id)}
                  onKeyDown={(event) => {
                    const next = event.key === "ArrowRight" ? (index + 1) % TABS.length
                      : event.key === "ArrowLeft" ? (index + TABS.length - 1) % TABS.length
                        : event.key === "Home" ? 0 : event.key === "End" ? TABS.length - 1 : null;
                    if (next === null) return;
                    event.preventDefault();
                    const tab = TABS[next]!;
                    panel.selectTab(tab.id);
                    document.getElementById(`right-sidebar-${tab.id}`)?.focus();
                  }}
                >
                  <Icon />
                </TitlebarButton>
              ))}
            </TitlebarTips>
          </div>
          <div
            id="right-sidebar-content"
            role="tabpanel"
            aria-labelledby={`right-sidebar-${panel.active}`}
            data-pane={panel.active}
            className={cn(
              "bg-sidebar text-sidebar-foreground rounded-e-page flex min-h-0 flex-1 flex-col overflow-hidden transition-opacity duration-150 motion-reduce:transition-none",
              panel.open ? "opacity-100" : "opacity-0",
            )}
          >
            {mounted && <Suspense fallback={null}>
              {panel.active === "files" ? <FilesTab conversationId={conversationId} searchRequest={panel.searchRequest} onSearchHandled={panel.searchHandled} />
                : panel.active === "source" ? <SourcePanel conversationId={conversationId} />
                  : <WorkersTab conversationId={conversationId} />}
            </Suspense>}
          </div>
        </div>
      </SidebarReveal>
      {panel.open && <SidebarResizeHandle {...panel} side="right" />}
    </div>
  );
}
