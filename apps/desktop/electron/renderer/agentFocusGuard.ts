// A driven click never takes the user's keyboard. A debugger press moves Chromium's focus into the guest, and only the
// app's page can move it back, so the element the user was in is put back after each input; and a press during IME
// composition is refused, since the blur would commit their unfinished text.
//
// The behaviour is Orca's src/renderer/src/components/browser-pane/host-guest/agent-input-focus-guard.ts and its main
// half (stablyai/orca at 10b8666, MIT, Copyright (c) 2026 Lovecast Inc.), adapted: our input is sent from this page, so
// there is no token handshake with main, and the page's focus goes back through the tab layer's handover.

/** The input's own focus move can reach this page just after it ends. */
const LATE_FOCUS_RECHECK_MS = 50;

let composingElement: EventTarget | null = null;
let inFlight = 0;
let userElement: Element | null = null;

/** Tracks the window's open composition. */
export function watchUserComposition(): void {
  document.addEventListener("compositionstart", (event) => { composingElement = event.target; }, true);
  // Nonempty updates can begin composition without a start; empty ones don't end an open candidate popup.
  document.addEventListener("compositionupdate", (event) => { if (event.data !== "") composingElement = event.target; }, true);
  document.addEventListener("compositionend", () => { composingElement = null; }, true);
  // Input outside a composition clears an ending that was missed.
  document.addEventListener("input", (event) => { if (!(event instanceof InputEvent && event.isComposing)) composingElement = null; }, true);
}

export const userComposing = (): boolean => composingElement !== null && composingElement === document.activeElement;

/** Holds the user's focus through an input to `page`; the answer puts it back. */
export function guardFocus(page: HTMLElement, giveBack: () => void): () => void {
  if (inFlight === 0) userElement = document.activeElement;
  inFlight += 1;
  const previous = userElement;
  const restore = () => {
    if (document.activeElement !== page) return;
    if (previous instanceof HTMLElement && previous !== page && previous !== document.body && previous.isConnected) previous.focus({ preventScroll: true });
    else giveBack();
  };
  return () => {
    restore();
    setTimeout(() => {
      restore();
      inFlight -= 1;
      if (inFlight === 0) userElement = null;
    }, LATE_FOCUS_RECHECK_MS);
  };
}
