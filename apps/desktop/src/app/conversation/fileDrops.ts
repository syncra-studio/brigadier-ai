import { useAui } from "@assistant-ui/react";
import { type DragEvent, useCallback, useContext, useEffect, useState } from "react";

import { whileReading } from "@/app/conversation/attachments";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { droppedFile, registerDropTarget } from "@/app/nativeDrop";
import { openFolders } from "@/state/addProject";
import { insertAtCaret } from "@/state/dictation";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { toast } from "@/state/toasts";

/**
 * Files dropped on the thread (`zone`) attach to the composer's message; with Shift held their
 * paths are typed at the composer's caret instead, and no "Drop to attach" shows. The page's own
 * drop (Linux, Windows) attaches through the dropzone, which `handlers` keep out of a Shift
 * drop; where the window takes drops from the page (macOS) they come by `registerDropTarget`,
 * and a folder among them still becomes a project.
 */
export function useFileDrops(zone: HTMLElement | null): {
  /** Files from the window's own drag are over the thread, to attach: the overlay shows. */
  native: boolean;
  /** Shift is held over the thread: no overlay. */
  shift: boolean;
  handlers: {
    onDragEnterCapture: (event: DragEvent) => void;
    onDragOverCapture: (event: DragEvent) => void;
    onDropCapture: (event: DragEvent) => void;
  };
} {
  const aui = useAui();
  const owner = useContext(ComposerTargetContext)?.conversation?.id ?? NEW_CHAT_SCOPE;
  // The window's own drag over the thread: with Shift held or not (`null`: none).
  const [native, setNative] = useState<boolean | null>(null);
  // The page's: whether its last drag event had Shift held. It stays so after a Shift drop,
  // which the dropzone never saw end, until the next drag comes in (the page's or the window's).
  const [shift, setShift] = useState(false);
  const typePaths = useCallback(
    (paths: string[]) => {
      if (paths.length > 0) insertAtCaret(owner, paths.join(" "));
    },
    [owner],
  );

  useEffect(() => {
    if (!zone) return;
    return registerDropTarget(zone, {
      hover: (held) => {
        setNative(held);
        if (held !== null) setShift(false);
      },
      drop: (entries, held) => {
        if (held) return typePaths(entries.map((entry) => entry.path));
        const folders = entries.filter((entry) => entry.folder).map((entry) => entry.path);
        if (folders.length > 0) void openFolders(folders);
        if (!aui.thread().getState().capabilities.attachments) return;
        for (const entry of entries) {
          if (entry.folder) continue;
          // Send waits until it's attached, so the file goes with this message (#330): through
          // the read, then until its tile, uploading, is in the composer (by the next task: its
          // first state comes in microtasks), whose upload holds Send from then on, or until
          // adding it failed. Removing the tile frees Send.
          const read = droppedFile(entry.path);
          const added = read.then((file) => aui.composer().addAttachment(file));
          const shown = read.then(() => Promise.race([added, new Promise((next) => setTimeout(next, 0))]));
          // A failed read or add is told below.
          whileReading(owner, shown).catch(() => {});
          added.catch((error: unknown) => toast(error instanceof Error ? error.message : String(error), { tone: "error" }));
        }
      },
    });
  }, [zone, aui, owner, typePaths]);

  const drag = useCallback((event: DragEvent) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    setShift(event.shiftKey);
    if (!event.shiftKey) return;
    // Handled here, so the dropzone neither shows its overlay nor attaches.
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  }, []);
  const drop = useCallback(
    (event: DragEvent) => {
      if (!event.dataTransfer.types.includes("Files") || !event.shiftKey) return;
      event.preventDefault();
      typePaths(droppedPaths(event.dataTransfer));
    },
    [typePaths],
  );

  return {
    native: native === false,
    shift: shift || native === true,
    handlers: { onDragEnterCapture: drag, onDragOverCapture: drag, onDropCapture: drop },
  };
}

/**
 * The paths of the files a page's drop holds, from its file URLs where the webview gives them
 * (WebKitGTK does); else their names, the most of a path the page has.
 */
function droppedPaths(data: DataTransfer): string[] {
  const urls = data
    .getData("text/uri-list")
    .split(/\r?\n/)
    .filter((line) => line.startsWith("file:"));
  if (urls.length === 0) return [...data.files].map((file) => file.name);
  return urls.map((url) => {
    const path = decodeURIComponent(new URL(url).pathname);
    // file:///C:/… on Windows.
    return /^\/[A-Za-z]:/.test(path) ? path.slice(1) : path;
  });
}
