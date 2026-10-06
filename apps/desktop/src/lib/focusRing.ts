/** Keys that move around the UI. Inside a text field only Tab counts: the rest edit text. */
const NAVIGATION_KEYS = new Set([
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Enter",
  " ",
]);

function editable(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || target.matches("input, textarea, select"))
  );
}

/**
 * Marks the root `data-input="pointer"` from a click until the keyboard moves around, so the
 * focus ring shows for the keyboard only (see styles/globals.css). WebKit counts focus that a
 * script moves after a click (a menu or dialog opening, the composer after New chat, a trigger
 * taking focus back) as keyboard focus, so `:focus-visible` alone would still ring it.
 */
export function trackFocusInput(): void {
  const root = document.documentElement;
  root.dataset.input = "pointer";
  window.addEventListener(
    "pointerdown",
    () => {
      root.dataset.input = "pointer";
    },
    true,
  );
  window.addEventListener(
    "keydown",
    (event) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const moves =
        event.key === "Tab" || (NAVIGATION_KEYS.has(event.key) && !editable(event.target));
      if (moves) delete root.dataset.input;
    },
    true,
  );
}
