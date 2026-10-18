import { FolderPlus } from "@openai/apps-sdk-ui/components/Icon";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useState } from "react";

import { type DropTarget, dragShift, dropTargetAt, lastDrop } from "@/app/nativeDrop";
import { openFolders } from "@/state/addProject";

/**
 * Folders dropped anywhere on the window become projects (see `openFolders`). While
 * something is dragged over the window, a veil says so. A drop that lands on a drop target
 * (the thread) is that target's instead (see `registerDropTarget`).
 */
export function FolderDropZone() {
  const [over, setOver] = useState(false);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | null = null;
    let paths: string[] = [];
    let hovered: DropTarget | null = null;
    let asking = false;
    // The drag's position is the window's, in points: the page's pixels.
    const hover = (x: number, y: number) => {
      const target = paths.length > 0 ? dropTargetAt(x, y) : null;
      if (target !== hovered) hovered?.hover(null);
      hovered = target;
      setOver(paths.length > 0 && target === null);
      if (!target || asking) return;
      asking = true;
      dragShift()
        .then((shift) => {
          if (hovered === target) target.hover(shift);
        })
        .catch((error: unknown) => console.error("drag shift unavailable", error))
        .finally(() => {
          asking = false;
        });
    };
    const drop = async (dropped: string[], x: number, y: number) => {
      hovered?.hover(null);
      hovered = null;
      paths = [];
      setOver(false);
      const target = dropTargetAt(x, y);
      if (!target) {
        if (dropped.length > 0) await openFolders(dropped);
        return;
      }
      const last = await lastDrop();
      target.drop(last?.entries ?? dropped.map((path) => ({ path, folder: false })), last?.shift ?? false);
    };
    getCurrentWebview()
      .onDragDropEvent(({ payload }) => {
        switch (payload.type) {
          case "enter":
            paths = payload.paths;
            hover(payload.position.x, payload.position.y);
            break;
          case "leave":
            hovered?.hover(null);
            hovered = null;
            paths = [];
            setOver(false);
            break;
          case "drop":
            void drop(payload.paths, payload.position.x, payload.position.y).catch((error: unknown) =>
              console.error("drop failed", error),
            );
            break;
          case "over":
            hover(payload.position.x, payload.position.y);
            break;
        }
      })
      .then((stop) => {
        if (live) unlisten = stop;
        else stop();
      })
      .catch((error: unknown) => console.error("folder drops unavailable", error));
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  if (!over) return null;
  return (
    <div
      aria-hidden
      className="bg-background/70 pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-6 backdrop-blur-sm"
    >
      <div className="border-foreground/25 rounded-dialog flex size-full flex-col items-center justify-center gap-3 border-2 border-dashed text-center">
        <FolderPlus className="text-muted-foreground size-10" />
        <p className="text-lg font-medium">Drop folders to add them as projects</p>
        <p className="text-muted-foreground text-sm">
          A repository is added right away. A folder that's already a project opens.
        </p>
      </div>
    </div>
  );
}
