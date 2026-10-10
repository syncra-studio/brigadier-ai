import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

import { takePaneClose } from "@/state/closedPanes";
import { useApp } from "@/state/store";
import { HOME_PLACE, toggleTerminal, undoTabClose } from "@/state/terminalPlaces";

/** The Panes and View menus' items (macOS), as the key presses the views listen for. */
export const MENU_KEYS: Record<string, { code: string; shift?: boolean; control?: boolean; alt?: boolean }> = {
  terminal: { code: "KeyJ" },
  "terminal-alternate": { code: "Backquote", control: true },
  new: { code: "KeyT" },
  reopen: { code: "KeyT", shift: true },
  address: { code: "KeyL" },
  "new-browser": { code: "KeyB", shift: true },
  "new-side-chat": { code: "KeyS", alt: true },
  "new-file": { code: "KeyN", alt: true },
  save: { code: "KeyS" },
  files: { code: "KeyP" },
  review: { code: "KeyG", control: true, shift: true },
  "cycle-next": { code: "Tab", control: true },
  "cycle-previous": { code: "Tab", control: true, shift: true },
  ...Object.fromEntries(Array.from({ length: 9 }, (_, index) => [`tab-${index + 1}`, { code: `Digit${index + 1}` }])),
  close: { code: "KeyW" },
  previous: { code: "BracketLeft", shift: true },
  next: { code: "BracketRight", shift: true },
  // The View menu's.
  back: { code: "BracketLeft" },
  forward: { code: "BracketRight" },
  sidebar: { code: "KeyB" },
  "right-sidebar": { code: "KeyB", alt: true },
};

/**
 * ⌘J shows or hides the bottom terminal. Sessions capture ⌃` to create a main terminal tab;
 * outside sessions it retains the bottom-terminal toggle,
 * and in Settings ⌘⇧T brings back Home's last closed terminal tab (a view does that for its
 * own). On macOS the Panes menu owns the keys; its items arrive here and go on as
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
      const browser = document.querySelector('[data-pane="browser"][data-active="true"]');
      const target = (!document.hasFocus() && browser) || document.activeElement || window;
      target.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          code: key.code,
          metaKey: !key.control,
          ctrlKey: key.control ?? false,
          shiftKey: key.shift ?? false,
          altKey: key.alt ?? false,
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
        // Elsewhere the view's own side panel reopens Home's terminals and pages.
        useApp.getState().selection.type === "settings" &&
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
