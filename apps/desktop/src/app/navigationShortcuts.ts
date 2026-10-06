import { useEffect } from "react";

import { stepHistory } from "@/state/actions";
import { useApp } from "@/state/store";

/** The mouse's back and forward side buttons, as `MouseEvent.button`. */
const MOUSE_BACK = 3;
const MOUSE_FORWARD = 4;

/**
 * Back and Forward through the places shown: ⌘[ and ⌘] (Ctrl off macOS; on macOS the View
 * menu owns them and its items arrive as these keys), and the mouse's side buttons. Not while
 * a dialog or menu is open.
 */
export function useNavigationShortcuts(): void {
  useEffect(() => {
    const blocked = () => document.querySelector('[role="dialog"], [role="menu"]') !== null;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      const mac = useApp.getState().info?.platform === "macos";
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!command || event.shiftKey || event.altKey) return;
      const delta = event.code === "BracketLeft" ? -1 : event.code === "BracketRight" ? 1 : 0;
      if (delta === 0) return;
      event.preventDefault();
      if (!blocked()) stepHistory(delta);
    };
    const onMouse = (event: MouseEvent) => {
      if (event.button !== MOUSE_BACK && event.button !== MOUSE_FORWARD) return;
      event.preventDefault();
      if (event.type === "mouseup" && !blocked()) {
        stepHistory(event.button === MOUSE_BACK ? -1 : 1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mousedown", onMouse);
    window.addEventListener("mouseup", onMouse);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mousedown", onMouse);
      window.removeEventListener("mouseup", onMouse);
    };
  }, []);
}
