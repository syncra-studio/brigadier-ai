import { type RefObject, useContext, useEffect, useEffectEvent } from "react";

import { ViewContext } from "@/app/conversation/viewContext";

/** A field the user types in; its keys are its own, not the card's. */
const FIELD =
  "input, textarea, select, [contenteditable]:not([contenteditable='false' i])";

/** Open menus, dialogs and the message field's popovers, which take Enter and Esc first. */
const OVERLAYS =
  "[role=dialog], [role=menu], [role=listbox], [data-slot=composer-commands], [data-slot=composer-mentions]";

export function inField(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(FIELD) !== null;
}

/**
 * Esc puts the card aside, and Enter answers it, from anywhere in its view: not from a text
 * field (the composer sends), not Enter on a focused button or link (that clicks it), and not
 * while a menu or dialog is open (they take their keys first). A side chat's keys are its own.
 * Without `enter` Enter is left alone, and with `within` it answers only from inside that
 * element (the focused card).
 */
export function useCardKeys(
  enabled: boolean,
  keys: { enter?: () => void; escape: () => void },
  within?: RefObject<HTMLElement | null>,
) {
  const { embedded } = useContext(ViewContext);
  const onKeyDown = useEffectEvent((event: globalThis.KeyboardEvent) => {
    if (event.key !== "Enter" && event.key !== "Escape") return;
    if (event.defaultPrevented || event.isComposing || event.repeat) return;
    if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey)
      return;
    const target = event.target instanceof Element ? event.target : null;
    if ((target?.closest("[data-embedded-view]") != null) !== embedded) return;
    if (inField(target)) return;
    if (event.key === "Enter") {
      if (!keys.enter) return;
      if (target?.closest("button, a[href], [role=button], summary")) return;
      if (within && !(target && within.current?.contains(target))) return;
    }
    if (document.querySelector(OVERLAYS)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Enter") keys.enter?.();
    else keys.escape();
  });
  useEffect(() => {
    if (!enabled) return;
    const listener = (event: globalThis.KeyboardEvent) => onKeyDown(event);
    // Capture, so the card answers before the composer's Esc-to-stop sees the key.
    window.addEventListener("keydown", listener, true);
    return () => window.removeEventListener("keydown", listener, true);
  }, [enabled]);
}
