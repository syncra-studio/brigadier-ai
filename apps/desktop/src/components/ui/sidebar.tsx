import * as React from "react";

import { TooltipProvider } from "@/components/ui/tooltip";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import {
  cachedCollapseMode,
  cachedOpen,
  saveCollapseMode,
  saveOpen,
  type SidebarCollapseMode,
} from "@/state/sidebar";
import { createPeek, PEEK_EXIT_MS, type Peek, type PeekPhase, type PeekZone } from "@/state/sidebarPeek";

/**
 * The sidebar panel: expanded to its width, collapsed to a strip of icons, or fully hidden.
 * It eases between these on a spring (the content beside it moves with it) while its contents
 * cross-fade. The expanded contents keep their full width, revealed rather than squeezed.
 * ⌘B or ⌘⇧S (Ctrl on Windows and Linux) toggle it, and its open state is remembered. The
 * strip/hidden choice is remembered too, under brigadier.sidebarCollapseMode. Its edge can be
 * dragged to resize it; dragging it below half its smallest width collapses it. The width is
 * remembered too. In a narrow window it collapses by itself, and expands again once there is
 * room if it was expanded before. While `keepOpen` (Settings) it is expanded whatever the choice,
 * and can't be toggled; the choice comes back with it.
 *
 * Collapsed, it peeks: resting the pointer on the strip's mark, or (hidden) on the window's start
 * edge, floats the expanded panel over the content until the pointer leaves it (state/sidebarPeek.ts
 * has the timing). Anything marked `data-sidebar-peek-trigger` peeks it. It never takes focus;
 * Escape, a click outside, resizing the window or navigating (`navigationKey` changing) put it
 * away, and it doesn't open while a menu or dialog is open.
 */

const WIDTH_KEY = "brigadier.sidebarWidth";

type SidebarContextProps = {
  state: "expanded" | "collapsed";
  open: boolean;
  collapseMode: SidebarCollapseMode;
  setCollapseMode: (mode: SidebarCollapseMode) => void;
  hidden: boolean;
  setOpen: (open: boolean) => void;
  toggleSidebar: () => void;
  /** The width the panel was dragged to, in CSS pixels; null for the default. */
  width: number | null;
  setWidth: (width: number | null) => void;
  resizing: boolean;
  setResizing: (resizing: boolean) => void;
  /** Whether it can collapse here (not while held open). */
  canToggle: boolean;
  /** The collapsed panel floating over the content: up, playing its exit, or not shown. */
  peek: PeekPhase;
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

function subscribeNarrow(onChange: () => void) {
  window.addEventListener("resize", onChange);
  return () => window.removeEventListener("resize", onChange);
}

/** Whether the window is too narrow for the expanded panel beside the content. */
function isNarrow() {
  return window.innerWidth < tokenPx("--spacing-narrow-window");
}

/** A menu, popover or dialog is open: the peek neither opens nor closes under it. */
function overlayOpen() {
  return document.querySelector('[role="menu"], [role="dialog"], [role="alertdialog"], [role="listbox"]') !== null;
}

/** After the window is resized, the peek waits this long before it can open again. */
const RESIZE_SETTLE_MS = 300;

/** What a pointer event is over, for the peek. */
function peekZone(event: PointerEvent): PeekZone {
  const target = event.target instanceof Element ? event.target : null;
  if (target?.closest("[data-sidebar-peek-panel]")) return "panel";
  // Only a resting mouse: not a touch, not a drag (selecting text, resizing) passing by. A
  // trigger out of use (the strip's mark while hidden) doesn't count.
  const trigger = target?.closest("[data-sidebar-peek-trigger]");
  if (event.pointerType === "mouse" && event.buttons === 0 && trigger && !trigger.closest("[inert]")) {
    return "trigger";
  }
  return "outside";
}

/**
 * The peek's pointer and keyboard wiring. `blocked` says whether it may open now (it is open for
 * real, or being resized).
 */
function usePeek(blocked: () => boolean, open: boolean, navigationKey: string | undefined): PeekPhase {
  const [phase, setPhase] = React.useState<PeekPhase>("closed");
  const peek = React.useRef<Peek | null>(null);
  const isBlocked = React.useRef(blocked);
  React.useEffect(() => {
    isBlocked.current = blocked;
  }, [blocked]);

  React.useEffect(() => {
    const fine = window.matchMedia("(hover: hover) and (pointer: fine)");
    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    let resizedAt = -Infinity;
    const controller = createPeek({
      onChange: (next) => {
        // Put away while focus is inside, it hands focus to the sidebar toggle, not to nowhere.
        const card = document.querySelector("[data-sidebar-peek-panel]");
        if (next !== "open" && card?.contains(document.activeElement)) {
          document.querySelector<HTMLElement>("[data-slot=sidebar-toggle]")?.focus({ preventScroll: true });
        }
        setPhase(next);
      },
      canOpen: () =>
        fine.matches &&
        !isBlocked.current() &&
        performance.now() - resizedAt > RESIZE_SETTLE_MS &&
        !overlayOpen(),
      holdOpen: overlayOpen,
      exitMs: () => (still.matches ? 0 : PEEK_EXIT_MS),
    });
    peek.current = controller;
    const onPointer = (event: PointerEvent) => controller.point(peekZone(event));
    const onLeave = () => controller.point("outside");
    const onPointerDown = (event: PointerEvent) => {
      if (controller.phase() !== "open" || overlayOpen()) return;
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest("[data-sidebar-peek-panel], [data-sidebar-peek-trigger]")) return;
      controller.dismiss();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      // A menu or dialog takes its own Escape first.
      if (event.key !== "Escape" || event.defaultPrevented || overlayOpen()) return;
      if (controller.phase() === "open") controller.dismiss();
    };
    const onResize = () => {
      resizedAt = performance.now();
      controller.dismiss();
    };
    document.addEventListener("pointermove", onPointer, { passive: true });
    document.addEventListener("pointerover", onPointer, { passive: true });
    document.documentElement.addEventListener("pointerleave", onLeave);
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", onResize);
    return () => {
      controller.dispose();
      peek.current = null;
      document.removeEventListener("pointermove", onPointer);
      document.removeEventListener("pointerover", onPointer);
      document.documentElement.removeEventListener("pointerleave", onLeave);
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", onResize);
    };
  }, []);

  // Opened for real, it is the panel itself again.
  React.useEffect(() => {
    if (open) peek.current?.reset();
  }, [open]);
  // Picking something in it (or going anywhere else) puts it away.
  const seenKey = React.useRef(navigationKey);
  React.useEffect(() => {
    if (seenKey.current === navigationKey) return;
    seenKey.current = navigationKey;
    peek.current?.dismiss();
  }, [navigationKey]);

  return open ? "closed" : phase;
}

function SidebarProvider({
  defaultOpen,
  keepOpen = false,
  navigationKey,
  className,
  style,
  children,
  ...props
}: React.ComponentProps<"div"> & { defaultOpen?: boolean; keepOpen?: boolean; navigationKey?: string }) {
  const narrow = React.useSyncExternalStore(subscribeNarrow, isNarrow);
  // The user's choice while the window has room (remembered), and while it is narrow
  // (collapsed on becoming narrow, so the panel doesn't crowd the content).
  const [wideOpen, setWideOpen] = React.useState(() => defaultOpen ?? cachedOpen());
  const [collapseMode, setCollapseModeState] = React.useState(cachedCollapseMode);
  const setCollapseMode = React.useCallback((mode: SidebarCollapseMode) => {
    setCollapseModeState(mode);
    saveCollapseMode(mode);
  }, []);
  const [narrowOpen, setNarrowOpen] = React.useState(false);
  const [seenNarrow, setSeenNarrow] = React.useState(narrow);
  if (seenNarrow !== narrow) {
    setSeenNarrow(narrow);
    setNarrowOpen(false);
  }
  React.useEffect(() => {
    if (defaultOpen === undefined) saveOpen(wideOpen);
  }, [defaultOpen, wideOpen]);
  const open = keepOpen || (narrow ? narrowOpen : wideOpen);
  const setChoice = narrow ? setNarrowOpen : setWideOpen;
  // Held open, the choice is left alone for when it lets go.
  const setOpen = React.useCallback(
    (next: boolean) => {
      if (!keepOpen) setChoice(next);
    },
    [keepOpen, setChoice],
  );
  const toggleSidebar = React.useCallback(() => {
    if (!keepOpen) setChoice((value) => !value);
  }, [keepOpen, setChoice]);

  const [width, setWidthState] = React.useState(cachedWidth);
  const setWidth = React.useCallback((next: number | null) => {
    setWidthState(next);
    saveWidth(next);
  }, []);
  const [resizing, setResizing] = React.useState(false);
  const peekBlocked = React.useCallback(() => open || resizing, [open, resizing]);
  const peek = usePeek(peekBlocked, open, navigationKey);

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

  const hidden = !open && collapseMode === "hidden";
  const state = open ? "expanded" : "collapsed";
  const contextValue = React.useMemo<SidebarContextProps>(
    () => ({
      state,
      open,
      collapseMode,
      setCollapseMode,
      hidden,
      setOpen,
      toggleSidebar,
      width,
      setWidth,
      resizing,
      setResizing,
      canToggle: !keepOpen,
      peek,
    }),
    [
      state, open, collapseMode, setCollapseMode, hidden, setOpen, toggleSidebar,
      width, setWidth, resizing, keepOpen, peek,
    ],
  );

  return (
    <SidebarContext.Provider value={contextValue}>
      <TooltipProvider delayDuration={0}>
      <div
        data-slot="sidebar-wrapper"
        data-state={state}
        data-collapse={hidden ? "hidden" : "strip"}
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
 * in the same place, their icons at the same spots; `foot` is shared by both, below them.
 * Collapsed, it can peek (see SidebarProvider): the expanded contents float beside the strip,
 * or at the window's start while hidden, with the foot too since no strip shows it.
 */
function SidebarPanel({
  strip,
  foot,
  className,
  children,
  ...props
}: React.ComponentProps<"div"> & { strip?: React.ReactNode; foot?: React.ReactNode }) {
  const { open, hidden, resizing, peek } = useSidebar();
  return (
    <div
      data-slot="sidebar-panel"
      data-state={open ? "expanded" : "collapsed"}
      className="relative flex h-full shrink-0"
    >
      <div
        inert={hidden}
        className={cn(
          "h-full overflow-hidden transition-[width] duration-300 ease-sidebar motion-reduce:transition-none",
          open ? "sidebar-panel-width" : hidden ? "w-0" : "w-sidebar-strip",
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
                inert={open || hidden}
                className={cn(
                  "w-sidebar-strip absolute inset-y-0 start-0 flex flex-col transition-opacity duration-150 motion-reduce:transition-none",
                  open || hidden ? "opacity-0" : "opacity-100",
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
      {peek !== "closed" && (
        <SidebarPeek closing={peek === "closing"} hidden={hidden} foot={hidden ? foot : null}>
          {children}
        </SidebarPeek>
      )}
      {/* Hidden, the window's start edge peeks it. */}
      {hidden && (
        <div
          aria-hidden
          data-sidebar-peek-trigger
          className="w-sidebar-peek-edge top-titlebar fixed start-0 bottom-0 z-20"
        />
      )}
    </div>
  );
}

/**
 * The collapsed panel peeking: its expanded contents over the content, which never moves for
 * it, carried on from the strip (or, hidden, on a card at the window's start). It floats in from the start over 300ms and fades back out over 200ms (at once
 * with reduced motion). Inside it, the contents lay out as in the expanded panel.
 */
function SidebarPeek({
  closing,
  hidden,
  foot,
  children,
}: {
  closing: boolean;
  hidden: boolean;
  foot: React.ReactNode;
  children: React.ReactNode;
}) {
  const context = useSidebar();
  const expanded = React.useMemo<SidebarContextProps>(
    () => ({ ...context, open: true, state: "expanded" }),
    [context],
  );
  return (
    <SidebarContext.Provider value={expanded}>
      <div
        inert={closing}
        data-slot="sidebar-peek"
        data-sidebar-peek-panel
        className={cn(
          "sidebar-panel-width text-sidebar-foreground bg-sidebar shadow-menu top-titlebar absolute bottom-0 z-40 flex flex-col overflow-hidden motion-reduce:animate-none",
          // Beside the strip it is the strip's own panel carried on: flush against it, square
          // where they meet with the strip's hairline kept, its edge and shadow on the far side
          // only. Hidden, it is a card.
          hidden
            ? "rounded-page ring-foreground/10 start-0 ring-1"
            : "rounded-e-page border-foreground/10 start-sidebar-strip start-hairline border-e clip-start",
          closing
            ? "pointer-events-none animate-[sidebar-peek-out_200ms_var(--ease-sidebar)_forwards]"
            : "animate-[sidebar-peek-in_300ms_var(--ease-sidebar)]",
        )}
      >
        <div className="relative flex min-h-0 flex-1 flex-col">{children}</div>
        {foot}
      </div>
    </SidebarContext.Provider>
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
