import { invoke } from "@tauri-apps/api/core";

/**
 * Places in the page that take what is dropped on them where the window takes drops from the
 * page (macOS: there the page sees no drag of files, nor their paths). `FolderDropZone` hears
 * every drop and hands one that lands on a target to it; elsewhere folders become projects.
 */
export type DropTarget = {
  /** Something is dragged over it (`shift`: with Shift held), or no longer (`null`). */
  hover(shift: boolean | null): void;
  /** Dropped on it: the paths, which of them are folders, and whether Shift was held. */
  drop(entries: DroppedEntry[], shift: boolean): void;
};

export type DroppedEntry = { path: string; folder: boolean };

const targets = new Map<Element, DropTarget>();

/** `element` takes the drops that land on it (or inside it); returns the undoing. */
export function registerDropTarget(element: Element, target: DropTarget): () => void {
  targets.set(element, target);
  return () => {
    if (targets.get(element) === target) targets.delete(element);
  };
}

/** The target at a point of the window (the drag's position, in the page's pixels), if any. */
export function dropTargetAt(x: number, y: number): DropTarget | null {
  for (let node = document.elementFromPoint(x, y); node; node = node.parentElement) {
    const target = targets.get(node);
    if (target) return target;
  }
  return null;
}

/** Whether Shift is held, as files are dragged over the window. */
export function dragShift(): Promise<boolean> {
  return invoke<boolean>("drag_shift");
}

/** The last drop on the window, as the window got it: Shift, and which paths are folders. */
export function lastDrop(): Promise<{ shift: boolean; entries: DroppedEntry[] } | null> {
  return invoke("last_drop");
}

/** A file of the last drop as a File to attach (typed by its extension, as a browser would). */
export async function droppedFile(path: string): Promise<File> {
  const bytes = await invoke<ArrayBuffer>("read_dropped", { path });
  const name = path.split(/[/\\]/).pop() ?? path;
  return new File([bytes], name, { type: MIME[name.split(".").pop()?.toLowerCase() ?? ""] ?? "" });
}

const MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  pdf: "application/pdf",
  txt: "text/plain",
  md: "text/markdown",
  csv: "text/csv",
  json: "application/json",
};
