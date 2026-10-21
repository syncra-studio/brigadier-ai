import { Counts } from "@/components/shared";
import type { FileStat } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** "+7 −2", uncoloured until the row is hovered; a dot for a created or deleted file. */
export function FileCounts({ file }: { file: FileStat }) {
  return (
    <span className="ms-1 flex shrink-0 items-center gap-1.5">
      {/* Read as words, the name and its counts stay apart. */}
      <span className="sr-only"> </span>
      {!file.binary && (
        <Counts data-slot="edited-file-counts" quiet insertions={file.insertions} deletions={file.deletions} />
      )}
      {(file.change === "add" || file.change === "delete") && (
        <span
          aria-hidden
          data-slot="edited-file-dot"
          data-change={file.change}
          className={cn("size-1.5 rounded-full", file.change === "add" ? "bg-link" : "bg-destructive")}
        />
      )}
    </span>
  );
}
