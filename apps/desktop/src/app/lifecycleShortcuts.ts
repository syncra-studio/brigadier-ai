import { useEffect } from "react";

import { archive, archiveAll, lastArchiveShown, undoLastArchive } from "@/state/actions";
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

/** When text was last typed anywhere (`performance.now()`). */
let typedAtMs = -Infinity;

/**
 * Whether ⌘Z at `target` belongs to its text: always outside an archive's Undo, and during one
 * when the field had focus at the archive or text was typed since. A field the archive moved
 * focus to (the composer of the conversation shown next) has nothing of its own to undo yet.
 */
function undoesText(target: EventTarget | null): boolean {
  if (!editsText(target)) return false;
  const last = lastArchiveShown();
  return !last || last.focus === target || typedAtMs > last.atMs;
}

/**
 * ⇧⌘A archives the picked sidebar rows, else the open conversation. ⌘⌫ asks to delete the
 * picked rows. ⌘Z right after an archive (while its toast shows) undoes it. Esc lets go of the
 * picked rows. (Ctrl for ⌘ on Windows and Linux.) ⌘⌫ and Esc leave text fields alone, and so
 * does ⌘Z where it has text to undo. None of them act while a dialog or menu is open.
 */
export function useLifecycleShortcuts(): void {
  useEffect(() => {
    const onInput = () => {
      typedAtMs = performance.now();
    };
    // Before the field's own handling, in a field the archive moved focus to.
    const onUndoInField = (event: KeyboardEvent) => {
      const mac = useApp.getState().info?.platform === "macos";
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey;
      if (!command || event.shiftKey || event.altKey || event.code !== "KeyZ") return;
      if (event.isComposing || !editsText(event.target) || undoesText(event.target)) return;
      if (document.querySelector('[role="dialog"], [role="menu"]') !== null) return;
      if (!undoLastArchive()) return;
      event.preventDefault();
      event.stopPropagation();
    };
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
        // Not behind a dialog or menu, where what it would archive can't be seen.
        if (open) return;
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
      if (event.shiftKey || open) return;
      if (typing) return;
      if (event.key === "Backspace" || event.key === "Delete") {
        if (picked.ids.length === 0 || useDeleteAsk.getState().ids) return;
        event.preventDefault();
        askDelete(picked.ids);
        return;
      }
      if (event.code === "KeyZ" && undoLastArchive()) event.preventDefault();
    };
    window.addEventListener("beforeinput", onInput, true);
    window.addEventListener("keydown", onUndoInField, true);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("beforeinput", onInput, true);
      window.removeEventListener("keydown", onUndoInField, true);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
}
