/*
 * For fixtures that pin a screen to the app's shared parts: how a surface is drawn, as the
 * browser computed it, and the colour a token class gives, read off a probe.
 */

/** A surface's fill, corners, edge and drop. */
export function surface(element: Element | null | undefined) {
  if (!element) return null;
  const css = getComputedStyle(element);
  return {
    background: css.backgroundColor,
    radius: css.borderTopLeftRadius,
    border: `${css.borderTopWidth} ${css.borderTopStyle} ${css.borderTopColor}`,
    shadow: css.boxShadow,
  };
}

/** The colour `className` gives `property` (a background by default). */
export function colourOf(className: string, property: "backgroundColor" | "color" = "backgroundColor"): string {
  const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className }));
  const colour = getComputedStyle(probe)[property];
  probe.remove();
  return colour;
}

/** Whether `row` spans its card from edge to edge, inside the card's border. */
export function fillsCard(row: Element | null | undefined): boolean {
  const card = row?.closest('[data-slot="settings-card"]');
  if (!row || !card) return false;
  const inner = card.getBoundingClientRect();
  const border = Number.parseFloat(getComputedStyle(card).borderLeftWidth);
  const box = row.getBoundingClientRect();
  return Math.abs(box.left - (inner.left + border)) < 0.5 && Math.abs(box.right - (inner.right - border)) < 0.5;
}

/** A switch's size and its fill when on (or off). */
export function switchLook(element: Element | null | undefined) {
  if (!element) return null;
  const box = element.getBoundingClientRect();
  return { width: box.width, height: box.height, background: getComputedStyle(element).backgroundColor };
}

export const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
