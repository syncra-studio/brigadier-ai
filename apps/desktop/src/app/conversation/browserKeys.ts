/**
 * The Browser tab's keys, and its page zoom's steps. On macOS the Browser menu owns the keys, so
 * they reach the tab also while its page has the keyboard (`MENU_KEYS` sends them on as these
 * presses); elsewhere they come from the app's own page.
 */

export type BrowserKey =
  | "address"
  | "back"
  | "forward"
  | "reload"
  | "force-reload"
  | "find"
  | "find-next"
  | "find-previous"
  | "zoom-in"
  | "zoom-out"
  | "zoom-reset";

type Press = Pick<KeyboardEvent, "code" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey">;

/**
 * What a key press asks of the browser tab in front, if anything: ⌘L the address, ⌘← and ⌘→
 * back and forward (Alt+← and Alt+→ off macOS), ⌘R reload and ⇧⌘R without the cache, ⌘F find
 * and ⌘G / ⇧⌘G the next and previous match, ⌘= (or ⌘+) / ⌘- / ⌘0 zoom. Ctrl stands for ⌘ off
 * macOS.
 */
export function browserKey(event: Press, mac: boolean): BrowserKey | null {
  const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!mac && event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
    if (event.code === "ArrowLeft") return "back";
    if (event.code === "ArrowRight") return "forward";
  }
  if (!command || event.altKey) return null;
  const shift = event.shiftKey;
  switch (event.code) {
    case "KeyL":
      return shift ? null : "address";
    case "ArrowLeft":
      return mac && !shift ? "back" : null;
    case "ArrowRight":
      return mac && !shift ? "forward" : null;
    case "KeyR":
      return shift ? "force-reload" : "reload";
    case "KeyF":
      return shift ? null : "find";
    case "KeyG":
      return shift ? "find-previous" : "find-next";
    // ⌘+ is ⌘= with Shift on most keyboards.
    case "Equal":
    case "NumpadAdd":
      return "zoom-in";
    case "Minus":
    case "NumpadSubtract":
      return shift ? null : "zoom-out";
    case "Digit0":
    case "Numpad0":
      return shift ? null : "zoom-reset";
    default:
      return null;
  }
}

/** The page zoom's least and most, 30% and 300%, in its 10% steps. */
export const ZOOM_LEAST = 0.3;
export const ZOOM_MOST = 3;

/** The zoom one 10% step in (`1`) or out (`-1`) from `zoom`, within 30% to 300%. */
export function stepZoom(zoom: number, by: 1 | -1): number {
  const tenths = Math.round(zoom * 10) + by;
  return Math.min(ZOOM_MOST * 10, Math.max(ZOOM_LEAST * 10, tenths)) / 10;
}

/** The zoom as the stepper shows it: "110%". */
export function zoomLabel(zoom: number): string {
  return `${Math.round(zoom * 100)}%`;
}
