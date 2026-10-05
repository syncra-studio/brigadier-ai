import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

import { takePaneClose } from "@/state/closedPanes";
import { useApp } from "@/state/store";
import { currentPlace, HOME_PLACE, toggleTerminal, undoTabClose } from "@/state/terminalPlaces";

/** The Panes menu's items (macOS), as the key presses the views listen for. */
const MENU_KEYS: Record<string, { code: string; shift?: boolean; control?: boolean }> = {
  terminal: { code: "KeyJ" },
  "terminal-alternate": { code: "Backquote", control: true },
  new: { code: "KeyT" },
  reopen: { code: "KeyT", shift: true },
  address: { code: "KeyL" },
  full: { code: "KeyF", shift: true },
  close: { code: "KeyW" },
  previous: { code: "BracketLeft", shift: true },
  next: { code: "BracketRight", shift: true },
};

/**
 * The panes' shortcuts, wherever the window is: ⌘J and ⌃` show or hide the bottom terminal,
 * and on Home and in Settings ⌘⇧T brings back its last closed tab (a conversation's view does
 * that for its own). On macOS the Panes menu owns the keys; its items arrive here and go on as
 * key presses, to the browser while it owns the keyboard.
 */
export function usePaneShortcuts(): void {
  const mac = useApp((s) => s.info?.platform === "macos");
  useEffect(() => {
    if (!mac) return;
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    void listen<string>("pane-shortcut", ({ payload }) => {
      const key = MENU_KEYS[payload];
      if (!key) return;
      const browser = document.querySelector('[data-pane="browser"]');
      const target = (!document.hasFocus() && browser) || document.activeElement || window;
      target.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          code: key.code,
          metaKey: !key.control,
          ctrlKey: key.control ?? false,
          shiftKey: key.shift ?? false,
        }),
      );
    })
      .then((off) => {
        if (disposed) off();
        else unsubscribe = off;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [mac]);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const command = mac ? event.metaKey : event.ctrlKey;
      const other = mac ? event.ctrlKey : event.metaKey;
      const toggle =
        (command && !other && !event.altKey && !event.shiftKey && event.code === "KeyJ") ||
        (event.ctrlKey &&
          !event.metaKey &&
          !event.altKey &&
          !event.shiftKey &&
          event.code === "Backquote");
      if (toggle) {
        event.preventDefault();
        toggleTerminal();
        return;
      }
      if (
        command &&
        event.shiftKey &&
        !event.altKey &&
        event.code === "KeyT" &&
        currentPlace() === HOME_PLACE &&
        takePaneClose(HOME_PLACE) === "terminal"
      ) {
        event.preventDefault();
        undoTabClose(HOME_PLACE);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac]);
}
