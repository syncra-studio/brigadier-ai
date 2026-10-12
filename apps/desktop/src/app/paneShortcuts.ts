import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

import { takePaneClose } from "@/state/closedPanes";
import { useApp } from "@/state/store";
import { HOME_PLACE, toggleTerminal, undoTabClose } from "@/state/terminalPlaces";

/** The Panes, Browser and View menus' items (macOS), as the key presses the views listen for. */
export const MENU_KEYS: Record<string, { code: string; shift?: boolean; control?: boolean; alt?: boolean }> = {
  terminal: { code: "KeyJ" },
  "terminal-alternate": { code: "Backquote", control: true },
  new: { code: "KeyT" },
  reopen: { code: "KeyT", shift: true },
  address: { code: "KeyL" },
  "new-browser": { code: "KeyB", shift: true },
  "new-side-chat": { code: "KeyS", alt: true },
  "new-page": { code: "KeyN", alt: true },
  save: { code: "KeyS" },
  "save-as": { code: "KeyS", shift: true },
  files: { code: "KeyP" },
  "show-pages": { code: "KeyE", alt: true },
  "cycle-next": { code: "Tab", control: true },
  "cycle-previous": { code: "Tab", control: true, shift: true },
  ...Object.fromEntries(Array.from({ length: 9 }, (_, index) => [`tab-${index + 1}`, { code: `Digit${index + 1}` }])),
  close: { code: "KeyW" },
  "close-others": { code: "KeyW", alt: true },
  previous: { code: "BracketLeft", shift: true },
  next: { code: "BracketRight", shift: true },
  "previous-tab": { code: "ArrowLeft", alt: true },
  "next-tab": { code: "ArrowRight", alt: true },
  // The Browser menu's, for the web page in front.
  "page-back": { code: "ArrowLeft" },
  "page-forward": { code: "ArrowRight" },
  reload: { code: "KeyR" },
  "force-reload": { code: "KeyR", shift: true },
  find: { code: "KeyF" },
  "find-next": { code: "KeyG" },
  "find-previous": { code: "KeyG", shift: true },
  "zoom-in": { code: "Equal" },
  "zoom-out": { code: "Minus" },
  "zoom-reset": { code: "Digit0" },
  // The View menu's.
  back: { code: "BracketLeft" },
  forward: { code: "BracketRight" },
  sidebar: { code: "KeyB" },
  "right-sidebar": { code: "KeyB", alt: true },
  "show-files": { code: "KeyE", shift: true },
  "show-source": { code: "KeyG", control: true, shift: true },
  // The app and File menus' own items, sent as keys only while Settings records a key press.
  settings: { code: "Comma" },
  "open-folder": { code: "KeyO" },
  "close-window": { code: "KeyW", shift: true },
};

/**
 * ⌘J shows or hides the bottom terminal. Live sessions capture ⌃` to create a main terminal tab;
 * outside them (and in archived sessions) it retains the bottom-terminal toggle,
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
