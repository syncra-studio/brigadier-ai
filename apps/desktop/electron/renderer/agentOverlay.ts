// What shows over a tab an agent drives: its named cursor and the take-over shield. Drawn in the app, never in the
// page: the page can't see it, and a picture of the page doesn't have it.
//
// The shield is the only way the user's input is told from the agent's. Debugger input is trusted to the page and
// runs through the same paths as a real click, so nothing can tell them apart after the fact. A plain element above
// the page takes the user's first click or key, says so, and goes; the agent's input goes under it. The layer lies
// under #root like the pages, so the app's own menus stay above it, and it is a sibling of the tab layer so the
// page layer's children stay exactly the pages. The cursor's colours are fixed, not the theme's: it lies over any site.
import type { BrowserBounds } from "@/ipc/generated";

type Overlay = {
  frame: HTMLDivElement;
  cursor: HTMLDivElement | null;
  name: HTMLSpanElement | null;
  at: { x: number; y: number } | null;
  placed: BrowserBounds | null;
  shielded: boolean;
  onUsed: () => void;
  onWheel: (at: { x: number; y: number; deltaX: number; deltaY: number }) => void;
};

const overlays = new Map<string, Overlay>();
let layer: HTMLDivElement | null = null;

export function installAgentOverlays(): void {
  const style = document.createElement("style");
  style.textContent = `
    #brigadier-agent-layer { position: fixed; inset: 0; z-index: 0; overflow: hidden; contain: strict; pointer-events: none; }
    .brigadier-agent-frame { position: absolute; overflow: hidden; outline: none; }
    .brigadier-agent-frame[data-shielded] { pointer-events: auto; }
    /* A menu of the app is open or a drag crosses the page: the click is the app's, as it is over a page. */
    #brigadier-tab-layer[data-locked] ~ #brigadier-agent-layer .brigadier-agent-frame { pointer-events: none; }
    .brigadier-agent-cursor { position: absolute; left: 0; top: 0; display: flex; align-items: flex-start; pointer-events: none; filter: drop-shadow(0 1px 2px rgb(0 0 0 / 0.35)); will-change: transform; }
    .brigadier-agent-name { margin: 14px 0 0 -3px; max-width: 180px; padding: 2px 7px; overflow: hidden; border-radius: 9px; background: #1a1a1a; color: #fff; box-shadow: 0 0 0 1px rgb(255 255 255 / 0.5); font: 500 11px/15px system-ui, sans-serif; white-space: nowrap; text-overflow: ellipsis; }
    .brigadier-agent-press { position: absolute; width: 26px; height: 26px; margin: -13px 0 0 -13px; border: 2px solid #1a1a1a; box-shadow: 0 0 0 1px rgb(255 255 255 / 0.7); border-radius: 50%; pointer-events: none; }
  `;
  document.head.append(style);
  layer = document.createElement("div");
  layer.id = "brigadier-agent-layer";
  document.getElementById("brigadier-tab-layer")?.after(layer);
  // The keyboard can't reach a page under a shield by Tab or by anything else in the app: the shield takes it.
  document.addEventListener("focusin", (event) => {
    if (!(event.target instanceof HTMLElement) || event.target.tagName !== "WEBVIEW") return;
    const tab = event.target.dataset.tab;
    const overlay = tab ? overlays.get(tab) : undefined;
    if (overlay?.shielded) overlay.frame.focus({ preventScroll: true });
  });
}

function placeFrame({ frame, placed }: Overlay): void {
  frame.style.display = placed ? "block" : "none";
  if (!placed) return;
  frame.style.left = `${placed.x}px`;
  frame.style.top = `${placed.y}px`;
  frame.style.width = `${placed.width}px`;
  frame.style.height = `${placed.height}px`;
}

/** A driven tab's overlay: `onUsed` hears the user's first click or key, `onWheel` their scrolling. */
export function raiseShield(id: string, page: HTMLElement, bounds: BrowserBounds | null, onUsed: () => void, onWheel: Overlay["onWheel"]): void {
  let overlay = overlays.get(id);
  if (!overlay) {
    const frame = document.createElement("div");
    frame.className = "brigadier-agent-frame";
    frame.tabIndex = -1;
    frame.dataset.agentTab = id;
    overlay = { frame, cursor: null, name: null, at: null, placed: bounds, shielded: false, onUsed, onWheel };
    overlays.set(id, overlay);
    const own = overlay;
    const take = (event: Event) => {
      if (!own.shielded) return;
      event.preventDefault();
      event.stopPropagation();
      own.onUsed();
    };
    frame.addEventListener("pointerdown", take);
    frame.addEventListener("keydown", take);
    frame.addEventListener("wheel", (event) => {
      if (!own.shielded) return;
      const rect = frame.getBoundingClientRect();
      own.onWheel({ x: event.clientX - rect.left, y: event.clientY - rect.top, deltaX: event.deltaX, deltaY: event.deltaY });
    }, { passive: true });
    layer?.append(frame);
  }
  overlay.onUsed = onUsed;
  overlay.onWheel = onWheel;
  overlay.placed = bounds;
  placeFrame(overlay);
  overlay.shielded = true;
  overlay.frame.toggleAttribute("data-shielded", true);
  // The keyboard was in the page: its first key is the user's too, so the shield holds it from here.
  if (document.activeElement === page) overlay.frame.focus({ preventScroll: true });
}

/** The shield goes and the cursor with it: the tab is the user's now, or no one drives it. */
export function dropOverlay(id: string): void {
  const overlay = overlays.get(id);
  if (!overlay) return;
  overlay.frame.remove();
  overlays.delete(id);
}

export function placeOverlay(id: string, bounds: BrowserBounds | null): void {
  const overlay = overlays.get(id);
  if (!overlay) return;
  overlay.placed = bounds;
  placeFrame(overlay);
}

const stillMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

const ARROW = `<svg width="20" height="22" viewBox="0 0 20 22" aria-hidden="true"><path d="M2 1.5 L2 17.5 L6.4 13.6 L9.3 20 L12.2 18.7 L9.4 12.4 L15.4 12.2 Z" fill="#1a1a1a" stroke="white" stroke-width="1.4" stroke-linejoin="round"/></svg>`;

function cursorOf(overlay: Overlay, label: string): HTMLDivElement {
  if (!overlay.cursor) {
    const cursor = document.createElement("div");
    cursor.className = "brigadier-agent-cursor";
    cursor.dataset.agentCursor = "";
    cursor.innerHTML = ARROW;
    const name = document.createElement("span");
    name.className = "brigadier-agent-name";
    cursor.append(name);
    overlay.frame.append(cursor);
    overlay.cursor = cursor;
    overlay.name = name;
  }
  if (overlay.name) overlay.name.textContent = label;
  return overlay.cursor;
}

const transformAt = ({ x, y }: { x: number; y: number }) => `translate(${x - 2}px, ${y - 1}px)`;

/**
 * The cursor glides to `to`, a point of the page, and this answers when it is there. Behind its tab the page shows
 * nothing, so nothing is waited for. Timed by the clock: a hidden window runs no animation to its end.
 */
export function glideTo(id: string, label: string, to: { x: number; y: number }): Promise<void> {
  const overlay = overlays.get(id);
  if (!overlay) return Promise.resolve();
  const cursor = cursorOf(overlay, label);
  const from = overlay.at ?? { x: (overlay.placed?.width ?? 0) / 2, y: (overlay.placed?.height ?? 0) * 0.15 };
  overlay.at = to;
  cursor.style.transform = transformAt(to);
  if (!overlay.placed) return Promise.resolve();
  if (stillMotion()) return Promise.resolve();
  // The same feel as the archive's agent_cursor: quick for a near control, unhurried across the page.
  const ms = Math.min(620, Math.max(220, 220 + Math.hypot(to.x - from.x, to.y - from.y) * 0.55));
  cursor.animate([{ transform: transformAt(from) }, { transform: transformAt(to) }], { duration: ms, easing: "cubic-bezier(0.22, 0.8, 0.24, 1)" });
  return new Promise((resolve) => setTimeout(resolve, ms + 40));
}

/** The press shows where the cursor is. */
export function pressAt(id: string): void {
  const overlay = overlays.get(id);
  if (!overlay?.at || !overlay.placed || stillMotion()) return;
  const ring = document.createElement("div");
  ring.className = "brigadier-agent-press";
  ring.style.left = `${overlay.at.x}px`;
  ring.style.top = `${overlay.at.y}px`;
  overlay.frame.append(ring);
  ring.animate([{ transform: "scale(0.4)", opacity: 0.9 }, { transform: "scale(1.3)", opacity: 0 }], { duration: 320, easing: "ease-out" }).finished.finally(() => ring.remove());
}
