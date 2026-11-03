import {
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

type Row = { id: string; element: HTMLElement; start: number; size: number; translate: string; transition: string; zIndex: string; position: string };
type Drag = { id: string; pointer: number; origin: number; at: number; to: number; rows: Row[]; scroll: number };

/** A single item, located by identity even when an optimistic or daemon update changed its slot. */
function moved<T>(items: readonly T[], idOf: (item: T) => string, id: string, to: number): readonly T[] {
  const item = items.find((item) => idOf(item) === id);
  if (!item) return items;
  const rest = items.filter((item) => idOf(item) !== id);
  rest.splice(to, 0, item);
  return rest;
}

/** Pointer reordering keeps the DOM order and pointer capture stable until release. Rows slide
 * into their proposed slots; the grabbed row follows the pointer without a transition. */
export function useDragReorder<T, E extends HTMLElement = HTMLElement>({
  items, idOf, rowSelector, onMove, axis = "y",
}: {
  items: readonly T[];
  idOf: (item: T) => string;
  rowSelector: string;
  onMove: (id: string, to: number) => void;
  axis?: "x" | "y";
}): {
  listRef: RefObject<E | null>;
  shown: readonly T[];
  dragging: string | null;
  grip: (id: string, index: number) => {
    onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
    onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
    onPointerUp: () => void;
    onPointerCancel: () => void;
    onLostPointerCapture: () => void;
    onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
  };
} {
  const listRef = useRef<E>(null);
  const active = useRef<Drag | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [pending, setPending] = useState<{ source: readonly T[]; id: string; to: number } | null>(null);
  const shown = pending?.source === items ? moved(items, idOf, pending.id, pending.to) : items;
  const latest = useRef({ items, shown, idOf, onMove });
  latest.current = { items, shown, idOf, onMove };

  const restore = (drag: Drag) => {
    for (const row of drag.rows) {
      row.element.style.translate = row.translate;
      row.element.style.transition = row.transition;
      row.element.style.zIndex = row.zIndex;
      row.element.style.position = row.position;
    }
  };
  const measure = (drag: Drag) => {
    const list = listRef.current;
    if (!list) return;
    restore(drag);
    drag.scroll = axis === "x" ? list.scrollLeft : list.scrollTop;
    drag.rows = [...list.querySelectorAll<HTMLElement>(`:scope > ${rowSelector}`)].flatMap((element, index) => {
      const item = latest.current.shown[index];
      if (!item) return [];
      const rect = element.getBoundingClientRect();
      return [{ id: latest.current.idOf(item), element, start: axis === "x" ? rect.left : rect.top,
        size: axis === "x" ? rect.width : rect.height, translate: element.style.translate,
        transition: element.style.transition, zIndex: element.style.zIndex, position: element.style.position }];
    });
  };
  const paint = (drag: Drag) => {
    const list = listRef.current;
    const grabbed = drag.rows.find((row) => row.id === drag.id);
    if (!list || !grabbed) return;
    const scroll = (axis === "x" ? list.scrollLeft : list.scrollTop) - drag.scroll;
    const delta = drag.at - drag.origin + scroll;
    const center = grabbed.start + grabbed.size / 2 + delta;
    const rtl = axis === "x" && getComputedStyle(list).direction === "rtl";
    const rest = drag.rows.filter((row) => row.id !== drag.id);
    drag.to = rest.filter((row) => rtl ? center < row.start + row.size / 2 : center > row.start + row.size / 2).length;
    const order = [...rest];
    order.splice(drag.to, 0, grabbed);
    let slot = drag.rows[0]?.start ?? 0;
    const gap = drag.rows.length > 1
      ? Math.abs(drag.rows[1]!.start - drag.rows[0]!.start) - (rtl ? drag.rows[1]!.size : drag.rows[0]!.size)
      : 0;
    const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
    for (const row of order) {
      // Across RTL lists the first row's right edge is the first slot's anchor.
      const target = rtl ? slot + (drag.rows[0]?.size ?? 0) - row.size : slot;
      const offset = row === grabbed ? delta : target - row.start;
      row.element.style.transition = row === grabbed || reduced ? "none" : "translate 160ms ease";
      row.element.style.translate = axis === "x" ? `${offset}px 0` : `0 ${offset}px`;
      if (row === grabbed) {
        row.element.style.position = "relative";
        row.element.style.zIndex = "10";
      }
      slot += (rtl ? -1 : 1) * (row.size + gap);
    }
  };
  const finish = (commit: boolean) => {
    const drag = active.current;
    if (!drag) return;
    active.current = null;
    restore(drag);
    setDragging(null);
    const current = latest.current;
    const from = current.items.findIndex((item) => current.idOf(item) === drag.id);
    if (commit && from >= 0 && from !== drag.to) {
      setPending({ source: current.items, id: drag.id, to: drag.to });
      current.onMove(drag.id, drag.to);
    }
  };

  // A live queue can add/remove/reorder rows during a drag. Measure the current identities,
  // never reinsert items[oldIndex] into the updated array (which duplicated another row).
  useLayoutEffect(() => {
    const drag = active.current;
    if (!drag) return;
    if (!items.some((item) => idOf(item) === drag.id)) { finish(false); return; }
    const grabbed = drag.rows.find((row) => row.id === drag.id);
    const visual = grabbed ? grabbed.start + drag.at - drag.origin : null;
    measure(drag);
    const next = drag.rows.find((row) => row.id === drag.id);
    if (visual !== null && next) drag.origin = drag.at - (visual - next.start);
    paint(drag);
  }, [items]);
  useLayoutEffect(() => () => {
    if (active.current) restore(active.current);
  }, []);

  const grip = (id: string, index: number) => ({
    onPointerDown: (event: ReactPointerEvent<HTMLElement>) => {
      if (event.button !== 0 || active.current) return;
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      const at = axis === "x" ? event.clientX : event.clientY;
      const drag: Drag = { id, pointer: event.pointerId, origin: at, at, to: index, rows: [], scroll: 0 };
      active.current = drag;
      measure(drag);
      paint(drag);
      setDragging(id);
    },
    onPointerMove: (event: ReactPointerEvent<HTMLElement>) => {
      const drag = active.current;
      if (!drag || drag.pointer !== event.pointerId) return;
      drag.at = axis === "x" ? event.clientX : event.clientY;
      paint(drag);
    },
    onPointerUp: () => finish(true),
    onPointerCancel: () => finish(false),
    onLostPointerCapture: () => finish(false),
    onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => {
      if (event.key === "Escape" && active.current) { event.preventDefault(); finish(false); return; }
      const [back, forward] = axis === "x" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
      const rtl = axis === "x" && getComputedStyle(event.currentTarget).direction === "rtl";
      const step = event.key === back ? -1 : event.key === forward ? 1 : 0;
      const to = index + (rtl ? -step : step);
      if (!step || to < 0 || to >= items.length) return;
      event.preventDefault();
      onMove(id, to);
    },
  });
  return { listRef, shown, dragging, grip };
}
