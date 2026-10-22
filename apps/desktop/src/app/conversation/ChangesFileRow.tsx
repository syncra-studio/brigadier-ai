import type { FC } from "react";

import { Button } from "@/components/controls";
import { ChevronRight, FileCode } from "@/components/icons";
import type { FileReview, FileStat } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** Where the review left a file, as its row says it; a merge says so once, in the card's header. */
const REVIEWED: Record<Exclude<FileReview, "pending">, string | null> = { later: "Changed later", kept: "Kept", undone: "Undone", merged: null };

/**
 * One file from a retained turn tree diff: its icon, the folder dim and the name in full, its counts in two columns
 * (right-aligned) and a chevron. A click on the row opens its diff in a tab (a file that waits for
 * review, its review). While it waits, the hovered row washes and Undo and Keep show before its
 * counts; once reviewed, where the review left it. The name is the row's button (Find rings it,
 * never marks it); the counts and chevron are a second one, out of the tab order, so Undo and Keep
 * can sit between them without a button in a button.
 */
export const ChangesFileRow: FC<{
  file: FileStat;
  review: FileReview | undefined;
  busy: boolean;
  onOpen: () => void;
  onAct: (what: "keep" | "undo") => void;
}> = ({ file, review, busy, onOpen, onAct }) => {
  const cut = file.path.lastIndexOf("/") + 1;
  const pending = review === "pending";
  return (
    <li
      data-slot="turn-diff-file"
      data-review={review}
      className={cn("group/file border-divider flex h-10 items-center gap-3 border-t", pending && "hover:bg-foreground/4 focus-within:bg-foreground/4")}
    >
      <button type="button" onClick={onOpen} title={file.path} className="flex h-full min-w-0 flex-1 items-center gap-3 text-start">
        <FileCode aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
        <span className={cn("min-w-0 flex-1 truncate text-label", review !== undefined && !pending && "opacity-60")}>
          <span data-slot="turn-diff-dir" className="text-muted-foreground">{file.path.slice(0, cut)}</span>
          <span data-slot="turn-diff-name" className="text-foreground font-medium">{file.path.slice(cut)}</span>
        </span>
        {review && !pending && REVIEWED[review] && (
          <span data-slot="turn-diff-reviewed" className="text-muted-foreground shrink-0 text-xs">{REVIEWED[review]}</span>
        )}
      </button>
      {pending && (
        <span data-slot="turn-diff-review-actions" className="hidden shrink-0 items-center gap-1 group-focus-within/file:flex group-hover/file:flex">
          <Button size="xs" variant="ghost" className="font-normal" disabled={busy} onClick={() => onAct("undo")}>Undo</Button>
          <Button size="xs" variant="ghost" className="font-normal" disabled={busy} onClick={() => onAct("keep")}>Keep</Button>
        </span>
      )}
      <button type="button" tabIndex={-1} aria-hidden onClick={onOpen} title={file.path} className="flex h-full shrink-0 items-center gap-3">
        {file.binary ? (
          <span className="text-muted-foreground w-17 shrink-0 text-end text-xs">binary</span>
        ) : (
          <span data-slot="turn-diff-file-counts" className="flex shrink-0 gap-2 font-mono text-xs tabular-nums">
            <span className={cn("w-7.5 text-end", file.insertions === 0 ? "text-muted-foreground" : "text-success")}>+{file.insertions}</span>{" "}
            <span className={cn("w-7.5 text-end", file.deletions === 0 ? "text-muted-foreground" : "text-destructive")}>−{file.deletions}</span>
          </span>
        )}
        <ChevronRight aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
      </button>
    </li>
  );
};
