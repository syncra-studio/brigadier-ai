import { useContext } from "react";

import { StepRow } from "@/app/conversation/activity/ActivityGroup";
import type { LeadStep } from "@/app/conversation/activity/group";
import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { leadAction, useLeadItems } from "@/app/conversation/activity/StepDetail";
import { basename, type StepWords, type ToolStep } from "@/app/conversation/activity/words";
import { OpenFileContext } from "@/components/assistant-ui/file-links";
import type { FileChangeKind, FileStat, ItemStatus } from "@/ipc/generated";
import { revealPath } from "@/ipc/client";
import { cn } from "@/lib/utils";

/**
 * A thread's own file change as rows, one per file: "Edited" with the file's name as a link that
 * opens it, then "+7 −2" (green and red while the row is hovered), a blue dot on a file it
 * created and a red one on a file it deleted. Each row opens to that file's diff, read with the
 * request's calls. A patch of several files lists each.
 */

const VERBS: Record<FileChangeKind, [doing: string, done: string]> = {
  add: ["Creating", "Created"],
  update: ["Editing", "Edited"],
  delete: ["Deleting", "Deleted"],
};

/** One changed file's words: "Edited math.js", "Created notes.md", "Rejected a.ts" when declined. */
export function fileWords(file: FileStat, status: ItemStatus): StepWords {
  const name = basename(file.path);
  const [doing, done] = VERBS[file.change ?? "update"];
  return {
    kind: "edit",
    doing: `${doing} ${name}`,
    done: `${status === "declined" ? "Rejected" : done} ${name}`,
    object: name,
    paths: [file.path],
  };
}

/** Whether a lead's step is a file change with its files counted: it shows as a row per file. */
export function editedFiles(step: LeadStep): FileStat[] | null {
  const { kind } = step;
  if (kind.type !== "tool" || kind.item !== "files" || !kind.files?.length) return null;
  return kind.files;
}

/** The file's name, underlined, opening the file: in its tab when it is the session's, else in Finder. */
function FileLink({ path, name }: { path: string; name: string }) {
  const openFile = useContext(OpenFileContext);
  return (
    <button
      type="button"
      data-slot="edited-file-link"
      title={path}
      onClick={(event) => {
        // The name opens the file; the rest of the row opens its diff.
        event.stopPropagation();
        if (!openFile(path, null)) revealPath(path).catch(() => {});
      }}
      onKeyDown={(event) => event.stopPropagation()}
      className="text-row-object hover:text-foreground focus-visible:text-foreground underline decoration-current/40 underline-offset-2"
    >
      {name}
    </button>
  );
}

/** "+7 −2", tabular and uncoloured until the row is hovered; a dot for a created or deleted file. */
function FileCounts({ file }: { file: FileStat }) {
  return (
    <span className="ms-1 flex shrink-0 items-center gap-1.5">
      {/* Read as words, the name and its counts stay apart. */}
      <span className="sr-only"> </span>
      {!file.binary && (
        <span data-slot="edited-file-counts" className="text-muted-foreground text-label tabular-nums">
          <span className="group-hover:text-success">+{file.insertions}</span>{" "}
          <span className="group-hover:text-destructive">−{file.deletions}</span>
        </span>
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

/** One file's diff, read from its call: the change its CLI applied, with its lines numbered. */
function FileDiff({ conversationId, requestId, kind, path }: { conversationId: string; requestId: string | null; kind: ToolStep; path: string }) {
  const { items, error } = useLeadItems(conversationId, requestId, kind);
  if (error && !items) return <p className="text-destructive text-xs">{error}</p>;
  if (!items) return <span className="shimmer text-xs">Loading</span>;
  const item = leadAction(items, kind);
  const changes = item?.kind === "files" ? item.changes.filter((change) => change.path === path && change.diff) : [];
  if (changes.length === 0) return <InlineDiff path={path} diff={null} />;
  return <>{changes.map((change, index) => <InlineDiff key={index} path={path} diff={change.diff} />)}</>;
}

/** A lead's file change, one row per file. */
export function EditedFileRows({ step, files, conversationId }: { step: LeadStep; files: readonly FileStat[]; conversationId: string | null }) {
  const { kind } = step;
  if (kind.type !== "tool") return null;
  return (
    <>
      {files.map((file) => {
        const words = fileWords(file, kind.status);
        return (
          <StepRow
            key={file.path}
            slot="edited-file"
            words={words}
            status={kind.status}
            startedAtMs={step.atMs}
            objectNode={<FileLink path={file.path} name={words.object ?? basename(file.path)} />}
            trail={kind.status === "inProgress" ? undefined : <FileCounts file={file} />}
            detail={conversationId && kind.status !== "inProgress" && kind.logSeq !== undefined
              ? <FileDiff conversationId={conversationId} requestId={step.requestId ?? null} kind={kind} path={file.path} />
              : undefined}
          />
        );
      })}
    </>
  );
}
