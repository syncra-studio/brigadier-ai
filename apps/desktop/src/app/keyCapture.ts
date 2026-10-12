import { invoke } from "@tauri-apps/api/core";

import { strokeOf } from "@/app/shortcutCatalog";

/** How long after the recorded press the menu's copy of it is swallowed. */
const ECHO_MS = 400;

/** Keeps a key from the app's handlers and the page. */
function swallow(event: KeyboardEvent) {
  event.preventDefault();
  event.stopImmediatePropagation();
}

/**
 * Records one key press (Settings › Keyboard shortcuts, search by keystrokes) ahead of the app's
 * handlers: a capture-phase listener on the window stops every key while it records, and the
 * shell sends the menu's own items (Settings…, Open Folder…, Close Window) here as keys rather
 * than acting. `onPress` gets the press in macOS notation, or null for Esc, which cancels.
 * Returns a function that stops recording.
 */
export function recordKeyPress(onPress: (stroke: string | null) => void): () => void {
  let recording = true;
  let echoesUntil = 0;
  let recorded: string | null = null;
  const remove = () => {
    window.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("keyup", onKeyUp, true);
  };
  const stop = () => {
    if (!recording) return;
    recording = false;
    echoesUntil = performance.now() + ECHO_MS;
    void invoke("set_key_capture", { on: false }).catch(() => {});
    setTimeout(remove, ECHO_MS);
  };
  function onKeyDown(event: KeyboardEvent) {
    if (!recording) {
      // Only the recorded key again (the menu's copy of it): another recording may have begun.
      if (performance.now() < echoesUntil && strokeOf(event) === recorded) swallow(event);
      return;
    }
    swallow(event);
    if (event.isComposing) return;
    const stroke = strokeOf(event);
    if (stroke === null) return;
    recorded = stroke;
    stop();
    onPress(stroke === "Esc" ? null : stroke);
  }
  function onKeyUp(event: KeyboardEvent) {
    if (recording) swallow(event);
  }
  window.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("keyup", onKeyUp, true);
  void invoke("set_key_capture", { on: true }).catch(() => {});
  return stop;
}
