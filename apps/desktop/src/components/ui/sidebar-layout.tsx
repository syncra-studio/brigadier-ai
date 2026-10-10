import * as React from "react";

import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { cachedWidth, saveWidth } from "@/state/sidebar";

function subscribeNarrow(onChange: () => void) {
  window.addEventListener("resize", onChange);
  return () => window.removeEventListener("resize", onChange);
}

/** Whether the window is too narrow for the expanded panel beside the content. */
function isNarrow() {
  return window.innerWidth < tokenPx("--spacing-narrow-window");
}

/** Shared width storage; each sidebar owns its key and CSS variable scope. */
export function useSidebarWidth(key: string) {
  const [width, setWidthState] = React.useState(() => cachedWidth(key));
  const setWidth = React.useCallback((next: number | null) => {
    setWidthState(next);
    saveWidth(key, next);
  }, [key]);
  const [resizing, setResizing] = React.useState(false);
  return { width, setWidth, resizing, setResizing };
}

/** Narrow windows temporarily fold the panel without changing the remembered wide choice. */
export function useSidebarChoice(
  wideOpen: boolean,
  setWideOpen: (next: boolean | ((current: boolean) => boolean)) => void,
  keepOpen = false,
  fold?: boolean,
) {
  const windowNarrow = React.useSyncExternalStore(subscribeNarrow, isNarrow);
  const narrow = fold ?? windowNarrow;
  const [narrowOpen, setNarrowOpen] = React.useState(false);
  const [seenNarrow, setSeenNarrow] = React.useState(narrow);
  if (seenNarrow !== narrow) {
    setSeenNarrow(narrow);
    setNarrowOpen(false);
  }
  const open = keepOpen || (narrow ? narrowOpen : wideOpen);
  const setChoice = narrow ? setNarrowOpen : setWideOpen;
  const setOpen = React.useCallback((next: boolean) => {
    if (!keepOpen) setChoice(next);
  }, [keepOpen, setChoice]);
  const toggleSidebar = React.useCallback(() => {
    if (!keepOpen) setChoice((value) => !value);
  }, [keepOpen, setChoice]);
  return { open, setOpen, toggleSidebar };
}

/** Both columns reveal full-width contents with the same spring as the left sidebar. */
export function SidebarReveal({ open, hidden, resizing, children, side = "left" }: {
  side?: "left" | "right";
  open: boolean;
  hidden: boolean;
  resizing: boolean;
  children: React.ReactNode;
}) {
  return (
    <div inert={hidden} className={cn(
      "h-full overflow-hidden transition-[width] duration-300 ease-sidebar motion-reduce:transition-none",
      open ? (side === "right" ? "right-sidebar-panel-width" : "sidebar-panel-width") : hidden ? "w-0" : "w-sidebar-strip",
      resizing && "transition-none",
    )}>
      {children}
    </div>
  );
}

/** The grab area straddling the panel's edge: drag to resize, below half the minimum to close. */
export function SidebarResizeHandle({
  setWidth, setOpen, resizing, setResizing, side = "left",
}: {
  setWidth: (width: number | null) => void;
  setOpen: (open: boolean) => void;
  resizing: boolean;
  setResizing: (resizing: boolean) => void;
  side?: "left" | "right";
}) {
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
    const moved = (event.clientX - start.current.x) * (rtl ? -1 : 1) * (side === "right" ? -1 : 1);
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
      ?.closest<HTMLElement>(side === "right" ? "[data-slot=right-sidebar]" : "[data-slot=sidebar-wrapper]")
      ?.style.setProperty(side === "right" ? "--right-sidebar-width" : "--sidebar-width", `${latest.current}px`);
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
      aria-label={side === "right" ? "Resize right sidebar" : "Resize sidebar"}
      data-slot="sidebar-resize-handle"
      data-resizing={resizing || undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      className={cn(
        "group/resize w-resize-handle top-titlebar absolute bottom-0 z-10 flex cursor-col-resize justify-center",
        side === "right" ? "start-0 -translate-x-1/2 rtl:translate-x-1/2" : "end-0 translate-x-1/2 rtl:-translate-x-1/2",
      )}
    >
      <span className="bg-input w-px opacity-0 transition-opacity duration-150 group-hover/resize:opacity-100 group-data-resizing/resize:opacity-100" />
    </div>
  );
}
