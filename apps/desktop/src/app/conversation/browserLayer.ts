import { browserLayer } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";

/**
 * On macOS a Browser tab's page lies beneath the app's own web view (see `src-tauri/src/
 * browser_ui.rs`). The app cuts a hole in itself where each page shows, so the page shows
 * through, while its menus, tooltips, popovers and dialogs, which render at the end of the body
 * outside the cut app, draw above the live page. The shell passes clicks in a hole to its page,
 * unless the app has a floating control there (`data-above-page`) or a menu, popover or dialog
 * open, which takes every click over a page (a click beside a menu closes it).
 */

/** App layers that take every click over a page while they are open. */
const MODAL = "[role=dialog], [role=alertdialog], [data-radix-popper-content-wrapper]";

/** A tooltip's content, which takes no clicks. */
const TOOLTIP = "[data-slot=tooltip-content], [role=tooltip]";

/** Where each tab's page shows now, by tab. */
const holes = new Map<string, BrowserBounds>();
let next = 0;
let cut = "";
let sent = "";

/** A popover that leaves the page live around it, taking clicks only on itself. */
const ABOVE = "[data-above-page]";

/**
 * Whether an open layer of the app takes clicks: any menu, popover or dialog, not a tooltip nor
 * a popover above the page (`data-above-page`, such as a page's question for the camera).
 */
export function modalOpen(layers: Iterable<Element>): boolean {
  for (const layer of layers) {
    if (layer.matches("[data-radix-popper-content-wrapper]") && layer.querySelector(TOOLTIP)) continue;
    if (layer.matches(ABOVE) || layer.querySelector(ABOVE)) continue;
    return true;
  }
  return false;
}

/**
 * The app's clip: the whole window but the holes, as a path whose window runs one way and each
 * hole the other, so a hole is cut however the holes overlap.
 */
export function clipPath(width: number, height: number, cuts: Iterable<BrowserBounds>): string {
  let path = `M0 0H${width}V${height}H0Z`;
  for (const { x, y, width: w, height: h } of cuts) {
    path += `M${x} ${y}V${y + h}H${x + w}V${y}Z`;
  }
  return `path(nonzero, "${path}")`;
}

const areaOf = (element: Element): BrowserBounds | null => {
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 ? { x: rect.left, y: rect.top, width: rect.width, height: rect.height } : null;
};

function sync() {
  next = 0;
  const root = document.getElementById("root");
  const shown = holes.size > 0;
  const clip = shown ? clipPath(window.innerWidth, window.innerHeight, holes.values()) : "";
  if (clip !== cut && root) {
    cut = clip;
    root.style.clipPath = clip;
    document.documentElement.toggleAttribute("data-page-beneath", shown);
  }
  const above = shown
    ? [...document.querySelectorAll(ABOVE)].map(areaOf).filter((area) => area !== null)
    : [];
  const modal = shown && modalOpen(document.querySelectorAll(MODAL));
  const layer = JSON.stringify([above, modal]);
  if (layer !== sent) {
    sent = layer;
    browserLayer(above, modal).catch(() => {});
  }
  // Menus open and floating controls move between page placements: watched every frame while
  // a page shows.
  if (shown) next = requestAnimationFrame(sync);
}

/** The tab's page shows at `bounds` (in the window), or no longer shows (`null`). */
export function showPage(id: string, bounds: BrowserBounds | null) {
  if (bounds) holes.set(id, bounds);
  else holes.delete(id);
  // While a page shows the next frame takes it; else at once.
  if (next === 0) sync();
}
