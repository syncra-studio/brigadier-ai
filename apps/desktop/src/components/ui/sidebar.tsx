import * as React from "react";

import { TooltipProvider } from "@/components/ui/tooltip";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";

/**
 * The sidebar panel: expanded to its width, or collapsed to a strip of icons. It eases between
 * the two on a spring (the content beside it moves with it) while its contents cross-fade; the
 * expanded contents keep their full width, so they are revealed rather than squeezed. ⌘B or
 * ⌘⇧S (Ctrl on Windows and Linux) toggle it, and which it is is remembered. Its edge can be
 * dragged to resize it; dragging it below half its smallest width collapses it. The width is
 * remembered too. In a narrow window it collapses by itself, and expands again once there is
 * room if it was expanded before.
 */

const WIDTH_KEY = "brigadier.sidebarWidth";
const COLLAPSED_KEY = "brigadier.sidebarCollapsed";

type SidebarContextProps = {
  state: "expanded" | "collapsed";
  open: boolean;
  setOpen: (open: boolean) => void;
  toggleSidebar: () => void;
  /** The width the panel was dragged to, in CSS pixels; null for the default. */
  width: number | null;
  setWidth: (width: number | null) => void;
  resizing: boolean;
  setResizing: (resizing: boolean) => void;
};

const SidebarContext = React.createContext<SidebarContextProps | null>(null);

function useSidebar() {
  const context = React.useContext(SidebarContext);
  if (!context) {
    throw new Error("useSidebar must be used within a SidebarProvider.");
  }
  return context;
}

function cachedWidth(): number | null {
  try {
    const width = Number(localStorage.getItem(WIDTH_KEY));
    return Number.isFinite(width) && width > 0 ? width : null;
  } catch {
    return null;
  }
}

function saveWidth(width: number | null): void {
  try {
    if (width === null) localStorage.removeItem(WIDTH_KEY);
    else localStorage.setItem(WIDTH_KEY, String(Math.round(width)));
  } catch {
    // Storage can be unavailable; the width then lasts until the app quits.
  }
}

function cachedOpen(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) !== "1";
  } catch {
    return true;
  }
}

function saveOpen(open: boolean): void {
  try {
    if (open) localStorage.removeItem(COLLAPSED_KEY);
    else localStorage.setItem(COLLAPSED_KEY, "1");
  } catch {
    // Storage can be unavailable; the sidebar then opens expanded on the next launch.
  }
}

function subscribeNarrow(onChange: () => void) {
  window.addEventListener("resize", onChange);
  return () => window.removeEventListener("resize", onChange);
}

/** Whether the window is too narrow for the expanded panel beside the content. */
function isNarrow() {
  return window.innerWidth < tokenPx("--spacing-narrow-window");
}

function SidebarProvider({
  defaultOpen,
  className,
  style,
  children,
  ...props
}: React.ComponentProps<"div"> & { defaultOpen?: boolean }) {
  const narrow = React.useSyncExternalStore(subscribeNarrow, isNarrow);
  // The user's choice while the window has room (remembered), and while it is narrow
  // (collapsed on becoming narrow, so the panel doesn't crowd the content).
  const [wideOpen, setWideOpen] = React.useState(() => defaultOpen ?? cachedOpen());
  const [narrowOpen, setNarrowOpen] = React.useState(false);
  const [seenNarrow, setSeenNarrow] = React.useState(narrow);
  if (seenNarrow !== narrow) {
    setSeenNarrow(narrow);
    setNarrowOpen(false);
  }
  React.useEffect(() => {
    if (defaultOpen === undefined) saveOpen(wideOpen);
  }, [defaultOpen, wideOpen]);
  const open = narrow ? narrowOpen : wideOpen;
  const setOpen = narrow ? setNarrowOpen : setWideOpen;
  const toggleSidebar = React.useCallback(() => setOpen((value) => !value), [setOpen]);

  const [width, setWidthState] = React.useState(cachedWidth);
  const setWidth = React.useCallback((next: number | null) => {
    setWidthState(next);
    saveWidth(next);
  }, []);
  const [resizing, setResizing] = React.useState(false);

  React.useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey || event.isComposing) return;
      const toggle =
        (event.code === "KeyB" && !event.shiftKey) || (event.code === "KeyS" && event.shiftKey);
      if (!toggle) return;
      event.preventDefault();
      if (!event.repeat) toggleSidebar();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [toggleSidebar]);

  const state = open ? "expanded" : "collapsed";
  const contextValue = React.useMemo<SidebarContextProps>(
    () => ({ state, open, setOpen, toggleSidebar, width, setWidth, resizing, setResizing }),
    [state, open, setOpen, toggleSidebar, width, setWidth, resizing],
  );

  return (
    <SidebarContext.Provider value={contextValue}>
      <TooltipProvider delayDuration={0}>
      <div
        data-slot="sidebar-wrapper"
        data-state={state}
        className={cn("group/sidebar-wrapper flex h-full w-full", className)}
        style={
          width === null ? style : ({ ...style, "--sidebar-width": `${width}px` } as React.CSSProperties)
        }
        {...props}
      >
        {children}
      </div>
      </TooltipProvider>
    </SidebarContext.Provider>
  );
}

/**
 * The sidebar panel: its column's top (in the titlebar strip) is left to the window chrome,
 * the rest is the panel's own surface. Only the column's width animates, between the panel's
 * width and the strip's. The expanded contents (`children`) and the strip's (`strip`) cross-fade
 * in the same place, their icons at the same spots; `foot` is shared by both, its labels
 * fading while collapsed.
 */
function SidebarPanel({
  strip,
  foot,
  className,
  children,
  ...props
}: React.ComponentProps<"div"> & { strip?: React.ReactNode; foot?: React.ReactNode }) {
  const { open, resizing } = useSidebar();
  return (
    <div data-slot="sidebar-panel" data-state={open ? "expanded" : "collapsed"} className="relative flex h-full shrink-0">
      <div
        className={cn(
          "h-full overflow-hidden transition-[width] duration-300 ease-sidebar motion-reduce:transition-none",
          open ? "sidebar-panel-width" : "w-sidebar-strip",
          resizing && "transition-none",
        )}
      >
        <div className="flex h-full flex-col">
          <div data-tauri-drag-region className="h-titlebar shrink-0" />
          <div
            data-sidebar="sidebar"
            className={cn(
              "text-sidebar-foreground bg-sidebar rounded-s-page flex min-h-0 flex-1 flex-col",
              className,
            )}
            {...props}
          >
            <div className="relative min-h-0 flex-1">
              <div
                inert={!open}
                className={cn(
                  "sidebar-panel-width absolute inset-y-0 start-0 flex flex-col transition-opacity duration-150 motion-reduce:transition-none",
                  open ? "opacity-100" : "opacity-0",
                )}
              >
                {children}
              </div>
              <div
                inert={open}
                className={cn(
                  "w-sidebar-strip absolute inset-y-0 start-0 flex flex-col transition-opacity duration-150 motion-reduce:transition-none",
                  open ? "opacity-0" : "opacity-100",
                )}
              >
                {strip}
              </div>
            </div>
            {foot}
          </div>
        </div>
      </div>
      {open && <SidebarResizeHandle />}
    </div>
  );
}

/** The grab area straddling the panel's edge: drag to resize, below half the minimum to close. */
function SidebarResizeHandle() {
  const { setWidth, setOpen, resizing, setResizing } = useSidebar();
  const start = React.useRef<{ x: number; width: number } | null>(null);
  const latest = React.useRef<number | null>(null);

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const panel = event.currentTarget.parentElement?.firstElementChild;
    if (!panel) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    start.current = { x: event.clientX, width: panel.getBoundingClientRect().width };
    latest.current = null;
    setResizing(true);
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    const rtl = getComputedStyle(event.currentTarget).direction === "rtl";
    const moved = (event.clientX - start.current.x) * (rtl ? -1 : 1);
    const wanted = start.current.width + moved;
    const min = tokenPx("--spacing-sidebar-min");
    const max = tokenPx("--spacing-sidebar-max");
    if (wanted < min / 2) {
      end(event);
      setOpen(false);
      return;
    }
    latest.current = Math.min(Math.max(wanted, min), max);
    event.currentTarget.parentElement
      ?.closest<HTMLElement>("[data-slot=sidebar-wrapper]")
      ?.style.setProperty("--sidebar-width", `${latest.current}px`);
  };

  const end = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    start.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    setResizing(false);
    if (latest.current !== null) setWidth(latest.current);
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      data-slot="sidebar-resize-handle"
      data-resizing={resizing || undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      className="group/resize w-resize-handle top-titlebar absolute bottom-0 end-0 z-10 flex translate-x-1/2 cursor-col-resize justify-center rtl:-translate-x-1/2"
    >
      <span className="bg-input w-px opacity-0 transition-opacity duration-150 group-hover/resize:opacity-100 group-data-resizing/resize:opacity-100" />
    </div>
  );
}

export { SidebarPanel, SidebarProvider, useSidebar };
