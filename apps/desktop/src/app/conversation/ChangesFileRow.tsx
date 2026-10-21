import type { FC } from "react";

import { Button } from "@/components/controls";
import { ChevronRight, FileCode } from "@/components/icons";
import type { FileReview, FileStat } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** Where the review left a file, as its row says it; a merge says so once, in the card's header. */
const REVIEWED: Record<Exclude<FileReview, "pending">, string | null> = { later: "Changed later", kept: "Kept", undone: "Undone", merged: null };

/**
 * One changed file: its icon, the folder dim and the name in full, its counts in two columns and
 * a chevron. A click opens its diff in a tab (a file that waits for review, its review). While it
 * waits, Undo and Keep show on hover; once reviewed, where the review left it.
 */
export const ChangesFileRow: FC<{
  file: FileStat;
  review: FileReview | undefined;
  busy: boolean;
  onOpen: () => void;
  onAct: (what: "keep" | "undo") => void;
}> = ({ file, review, busy, onOpen, onAct }) => {
  const cut = file.path.lastIndexOf("/") + 1;
  return (
    <li data-slot="turn-diff-file" data-review={review} className="group/file border-divider flex h-10 items-center gap-3 border-t">
      <button type="button" onClick={onOpen} title={file.path} className="flex h-full min-w-0 flex-1 items-center gap-3 text-start">
        <FileCode aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
        <span className={cn("min-w-0 flex-1 truncate text-label", review !== undefined && review !== "pending" && "opacity-60")}>
          <span data-slot="turn-diff-dir" className="text-muted-foreground">{file.path.slice(0, cut)}</span>
          <span data-slot="turn-diff-name" className="text-foreground font-medium">{file.path.slice(cut)}</span>
        </span>
        {review && review !== "pending" && REVIEWED[review] && (
          <span data-slot="turn-diff-reviewed" className="text-muted-foreground shrink-0 text-xs">{REVIEWED[review]}</span>
        )}
        {file.binary ? (
          <span className="text-muted-foreground w-17 shrink-0 text-xs">binary</span>
        ) : (
          <span data-slot="turn-diff-file-counts" className="flex shrink-0 gap-2 font-mono text-xs tabular-nums">
            <span className={cn("w-7.5", file.insertions === 0 ? "text-muted-foreground" : "text-success")}>+{file.insertions}</span>{" "}
            <span className={cn("w-7.5", file.deletions === 0 ? "text-muted-foreground" : "text-destructive")}>−{file.deletions}</span>
          </span>
        )}
        <ChevronRight aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
      </button>
      {review === "pending" && (
        <span className="hidden shrink-0 items-center gap-1 group-focus-within/file:flex group-hover/file:flex">
          <Button size="xs" variant="ghost" className="font-normal" disabled={busy} onClick={() => onAct("undo")}>Undo</Button>
          <Button size="xs" variant="ghost" className="font-normal" disabled={busy} onClick={() => onAct("keep")}>Keep</Button>
        </span>
      )}
    </li>
  );
};
