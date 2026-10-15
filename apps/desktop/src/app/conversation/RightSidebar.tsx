import { Lightbulb, X } from "@openai/apps-sdk-ui/components/Icon";
import { type CSSProperties, createContext, lazy, Suspense, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { WORKERS_LABEL, WorkersTab } from "@/app/conversation/Agents";
import { outcomeWords } from "@/app/conversation/workerPresentation";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { SidebarReveal, SidebarResizeHandle, useSidebarChoice, useSidebarWidth } from "@/components/ui/sidebar-layout";
import { useSidebar } from "@/components/ui/sidebar";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { rightSidebarFolds, type RightSidebarTab, selectRightSidebarTab, setRightSidebarOpen, useRightSidebarState } from "@/state/rightSidebar";
import { useBoard } from "@/state/board";
import { usePlanTab } from "@/state/planDoc";

const PlanTab = lazy(() => import("@/app/conversation/PlanTab").then((m) => ({ default: m.PlanTab })));
const TABS = [{ id: "workers", title: WORKERS_LABEL, Icon: null }] as const;
/** Plan joins the tabs only after the user opens a plan. */
const WITH_PLAN = [...TABS, { id: "plan", title: "Plan", Icon: Lightbulb }] as const;

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
  const [openedFor, setOpenedFor] = useState<string | null>(null);
  const wideOpen = useRightSidebarState((s) => s.open);
  const active = useRightSidebarState((s) => conversationId ? s.tabs[conversationId] ?? "workers" : "workers");
  const { open, setOpen } = useSidebarChoice(wideOpen, setRightSidebarOpen, false, fold);
  const { width, setWidth, resizing, setResizing } = useSidebarWidth("brigadier.rightSidebarWidth");
  const selectTab = useCallback((tab: RightSidebarTab) => {
    if (enabled && conversationId) selectRightSidebarTab(conversationId, tab);
  }, [conversationId, enabled]);
  const openTab = useCallback((tab: RightSidebarTab) => {
    if (!enabled || !conversationId) return;
    selectTab(tab);
    setOpenedFor(conversationId);
    setOpen(true);
  }, [enabled, conversationId, selectTab, setOpen]);
  return useMemo(() => ({
    width, setWidth, resizing, setResizing, enabled, open: enabled && open && openedFor === conversationId, active,
    setOpen, selectTab, openTab,
  }), [width, setWidth, resizing, setResizing, enabled, open, openedFor, conversationId, active, setOpen, selectTab, openTab]);
}

export const RightSidebarContext = createContext<ReturnType<typeof useRightSidebar> | null>(null);

/**
 * The Workers and Plan column, below the titlebar: its tabs and close in a row at its top.
 * Closing reveals no strip and installs no hover trigger.
 */
export function RightSidebar({ conversationId }: { conversationId: string }) {
  const panel = useContext(RightSidebarContext);
  const { panel: workerId } = useContext(AgentsPanelContext);
  const selectedWorker = useBoard((s) => !!workerId && !!s.board?.tasks[workerId]);
  const workerHeader = panel?.active === "workers" && !!selectedWorker;
  const content = useRef<HTMLDivElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const [tabFocus, setTabFocus] = useState<RightSidebarTab | null>(null);
  const selectTab = (tab: RightSidebarTab) => {
    setTabFocus(tab);
    panel?.selectTab(tab);
  };
  // Switching to a worker removes the tabs; switching to Plan removes its header button.
  // Focus the replacement only after React has mounted it.
  useLayoutEffect(() => {
    const tab = tabFocus;
    if (tab === null) return;
    const target = content.current?.querySelector<HTMLElement>(
      workerHeader ? '[data-slot="worker-header"] button' : `#right-sidebar-${tab}`,
    );
    if (target) {
      target.focus({ preventScroll: true });
      setTabFocus(null);
    }
  }, [tabFocus, workerHeader]);
  const column = useRef<HTMLDivElement>(null);
  const tabs: readonly (typeof WITH_PLAN)[number][] = usePlanTab((s) => s.shown[conversationId] !== undefined) ? WITH_PLAN : TABS;
  // Finished workers leave the composer's strip once answered; they stay here, counted.
  // "1 done · 1 stopped": a stopped worker never counts as done (BUILD-PLAN A7).
  const finished = useBoard((s) =>
    s.board?.conversationId === conversationId ? outcomeWords(Object.values(s.board.tasks)) : "");
  // The left column and the docked pane leave room for where this column ends up, as this one
  // does for the left column: its full width while open, none while closed. That width is read
  // off the contents, which keep it while the column's width animates, so opening and closing
  // write it once rather than restyling the whole window on every frame of the slide.
  const open = panel?.open ?? false;
  useLayoutEffect(() => {
    const element = content.current;
    const left = column.current?.closest<HTMLElement>('[data-slot="sidebar-wrapper"]');
    if (!open || !element || !left) return;
    const measure = () => left.style.setProperty("--right-sidebar-shown", `${element.getBoundingClientRect().width}px`);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
      left.style.removeProperty("--right-sidebar-shown");
    };
  }, [open]);
  const [mounted, setMounted] = useState(panel?.open ?? false);
  if (panel?.open && !mounted) setMounted(true);
  useEffect(() => {
    if (panel?.open) {
      returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    } else if (content.current?.contains(document.activeElement)) {
      returnFocus.current?.focus({ preventScroll: true });
    }
  }, [panel?.open]);
  if (!panel?.enabled) return null;
  return (
    <div
      ref={column}
      data-slot="right-sidebar"
      data-state={panel.open ? "expanded" : "collapsed"}
      className={cn(
        "start-hairline relative flex h-full shrink-0 before:transition-opacity before:duration-300 before:ease-sidebar motion-reduce:before:transition-none",
        !panel.open && "before:opacity-0",
      )}
      style={{ "--right-sidebar-width": panel.width === null ? "var(--spacing-sidebar)" : `${panel.width}px` } as CSSProperties}
    >
      <SidebarReveal side="right" open={panel.open} hidden={!panel.open} resizing={panel.resizing}>
        <div ref={content} className="right-sidebar-panel-width flex h-full flex-col">
          {!workerHeader && <div role="tablist" aria-label="Right panel" className="h-page-toolbar bg-background flex shrink-0 items-center gap-1 px-2">
            <TitlebarTips>
              {tabs.map(({ id, title, Icon }, index) => (
                <TitlebarButton
                  key={id}
                  role="tab"
                  id={`right-sidebar-${id}`}
                  aria-controls="right-sidebar-content"
                  aria-selected={panel.active === id}
                  className={cn("aria-selected:bg-toolbar-pressed aria-selected:text-foreground", id === "workers" && "w-auto px-2 text-label")}
                  tabIndex={panel.active === id ? 0 : -1}
                  tooltip={id === "workers" && finished ? `${title} · ${finished}` : title}
                  onClick={() => selectTab(id)}
                  onKeyDown={(event) => {
                    const next = event.key === "ArrowRight" ? (index + 1) % tabs.length
                      : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length
                        : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : null;
                    if (next === null) return;
                    event.preventDefault();
                    const tab = tabs[next]!;
                    selectTab(tab.id);
                  }}
                >
                  {Icon ? <Icon /> : title}
                </TitlebarButton>
              ))}
            </TitlebarTips>
            <span className="flex-1" />
            <TitlebarTips>
              <TitlebarButton tooltip="Close right panel" onClick={() => panel.setOpen(false)}><X /></TitlebarButton>
            </TitlebarTips>
          </div>}
          <div
            id="right-sidebar-content"
            role={workerHeader ? "region" : "tabpanel"}
            aria-labelledby={workerHeader ? undefined : `right-sidebar-${panel.active}`}
            aria-label={workerHeader ? "Workers" : undefined}
            data-pane={panel.active}
            className={cn(
              "bg-background text-sidebar-foreground flex min-h-0 flex-1 flex-col overflow-clip transition-opacity duration-150 motion-reduce:transition-none",
              panel.open ? "opacity-100" : "opacity-0",
            )}
          >
            {mounted && <Suspense fallback={null}>
              {panel.active === "plan" ? <PlanTab conversationId={conversationId} /> : <WorkersTab conversationId={conversationId} headerActions={<TitlebarTips>
                {tabs.length > 1 && <TitlebarButton tooltip="Plan" onClick={() => selectTab("plan")}><Lightbulb /></TitlebarButton>}
                <TitlebarButton tooltip="Close right panel" onClick={() => panel.setOpen(false)}><X /></TitlebarButton>
              </TitlebarTips>} />}
            </Suspense>}
          </div>
        </div>
      </SidebarReveal>
      {panel.open && <SidebarResizeHandle {...panel} side="right" />}
    </div>
  );
}
