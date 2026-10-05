import {
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useRef,
  useState,
} from "react";

type Drag = { id: string; from: number; to: number };

/** Where a dragged row would land, from the pointer's place over the rows' midpoints along
 * `axis` (across, a right-to-left list counts from its right). */
function dropIndex(
  list: HTMLElement,
  rowSelector: string,
  at: number,
  dragged: number,
  axis: "x" | "y",
): number {
  const rows = [...list.querySelectorAll<HTMLElement>(`:scope > ${rowSelector}`)];
  const rtl = axis === "x" && getComputedStyle(list).direction === "rtl";
  let index = 0;
  for (const [position, row] of rows.entries()) {
    if (position === dragged) continue;
    const rect = row.getBoundingClientRect();
    const middle = axis === "x" ? rect.left + rect.width / 2 : rect.top + rect.height / 2;
    if (rtl ? at < middle : at > middle) index++;
  }
  return index;
}

/**
 * A list the user reorders by a grip on each row: dragged with the pointer (rows show in the
 * order they would drop in meanwhile), or moved one place with ↑ and ↓ while the grip has focus
 * (← and → for a list across, `axis` "x"). `onMove` gets the row's id and the index it lands at.
 */
export function useDragReorder<T, E extends HTMLElement = HTMLElement>({
  items,
  idOf,
  rowSelector,
  onMove,
  axis = "y",
}: {
  items: readonly T[];
  idOf: (item: T) => string;
  /** Selects the list's direct children that are rows. */
  rowSelector: string;
  onMove: (id: string, to: number) => void;
  axis?: "x" | "y";
}): {
  listRef: RefObject<E | null>;
  /** The items in the order to show them. */
  shown: readonly T[];
  /** The id of the row being dragged. */
  dragging: string | null;
  /** Handlers for a row's grip. */
  grip: (id: string, index: number) => {
    onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
    onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
    onPointerUp: () => void;
    onPointerCancel: () => void;
    onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
  };
} {
  const [drag, setDrag] = useState<Drag | null>(null);
  const listRef = useRef<E>(null);
  const shown = drag
    ? (() => {
        const order = items.filter((item) => idOf(item) !== drag.id);
        const moved = items[drag.from];
        if (moved !== undefined) order.splice(drag.to, 0, moved);
        return order;
      })()
    : items;

  const grip = (id: string, index: number) => ({
    onPointerDown: (event: ReactPointerEvent<HTMLElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      setDrag({ id, from: index, to: index });
    },
    onPointerMove: (event: ReactPointerEvent<HTMLElement>) => {
      if (!drag || !listRef.current) return;
      const dragged = shown.findIndex((item) => idOf(item) === drag.id);
      const to = dropIndex(
        listRef.current,
        rowSelector,
        axis === "x" ? event.clientX : event.clientY,
        dragged,
        axis,
      );
      if (to !== drag.to) setDrag({ ...drag, to });
    },
    onPointerUp: () => {
      if (!drag) return;
      setDrag(null);
      if (drag.to !== drag.from) onMove(drag.id, drag.to);
    },
    onPointerCancel: () => setDrag(null),
    onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => {
      const [back, forward] = axis === "x" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
      const rtl = axis === "x" && getComputedStyle(event.currentTarget).direction === "rtl";
      const step = event.key === back ? -1 : event.key === forward ? 1 : 0;
      const to = step === 0 ? null : index + (rtl ? -step : step);
      if (to === null || to < 0 || to >= items.length) return;
      event.preventDefault();
      onMove(id, to);
    },
  });

  return { listRef, shown, dragging: drag?.id ?? null, grip };
}
