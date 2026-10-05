import { useEffect } from "react";

import { archive, archiveAll, undoLastArchive } from "@/state/actions";
import { askDelete, clearPicked, useDeleteAsk, usePicked } from "@/state/picking";
import { useApp } from "@/state/store";

/** Whether keys typed at `target` edit text there (where ⌘⌫ and ⌘Z belong to the text). */
function editsText(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable || target instanceof HTMLTextAreaElement) return true;
  return (
    target instanceof HTMLInputElement &&
    !["button", "checkbox", "color", "file", "image", "radio", "range", "reset", "submit"].includes(
      target.type,
    )
  );
}

/**
 * ⇧⌘A archives the picked sidebar rows, else the open conversation. ⌘⌫ asks to delete the
 * picked rows. ⌘Z right after an archive (while its toast shows) undoes it. Esc lets go of the
 * picked rows. (Ctrl for ⌘ on Windows and Linux.) ⌘⌫, ⌘Z and Esc leave text fields alone.
 */
export function useLifecycleShortcuts(): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      const state = useApp.getState();
      const mac = state.info?.platform === "macos";
      const command = mac ? event.metaKey : event.ctrlKey;
      const typing = editsText(event.target);
      const picked = usePicked.getState();
      const open = document.querySelector('[role="dialog"], [role="menu"]') !== null;
      if (event.key === "Escape" && !command && !event.shiftKey && !event.altKey) {
        if (picked.ids.length > 0 && !typing && !open) clearPicked();
        return;
      }
      if (!command || event.altKey || (mac && event.ctrlKey)) return;
      if (event.shiftKey && event.code === "KeyA") {
        event.preventDefault();
        if (picked.list === "sidebar" && picked.ids.length > 0) {
          const ids = picked.ids;
          clearPicked();
          void archiveAll(ids);
          return;
        }
        const { selection } = state;
        if (selection.type !== "conversation") return;
        if (state.conversations[selection.id]?.lifecycle === "archived") return;
        void archive(selection.id);
        return;
      }
      if (typing || event.shiftKey || open) return;
      if (event.key === "Backspace" || event.key === "Delete") {
        if (picked.ids.length === 0 || useDeleteAsk.getState().ids) return;
        event.preventDefault();
        askDelete(picked.ids);
        return;
      }
      if (event.code === "KeyZ" && undoLastArchive()) event.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
