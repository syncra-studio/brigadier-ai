import { type ReactNode, useContext } from "react";

import { StepRow } from "@/app/conversation/activity/ActivityGroup";
import type { LeadStep } from "@/app/conversation/activity/group";
import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { leadAction, useLeadItems } from "@/app/conversation/activity/StepDetail";
import { basename, type StepWords, type ToolStep } from "@/app/conversation/activity/words";
import { Counts } from "@/app/conversation/Counts";
import { OpenFileContext } from "@/components/conversation";
import type { FileChange, FileChangeKind, FileStat, ItemStatus } from "@/ipc/generated";
import { revealPath } from "@/ipc/client";
import { diffCounts, parseDiff } from "@/lib/patch";
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

/**
 * One changed file's words: "Edited math.js", "Created notes.md", "Renamed a.rs to b.rs" (a move
 * that keeps the name "Moved b.rs"), "Rejected a.ts" when declined.
 */
export function fileWords(file: FileStat, status: ItemStatus): StepWords {
  const name = basename(file.path);
  const was = file.from ? basename(file.from) : null;
  const [doing, done] = was === null ? VERBS[file.change ?? "update"] : was === name ? ["Moving", "Moved"] : ["Renaming", "Renamed"];
  const what = was === null || was === name ? name : `${was} to ${name}`;
  return {
    kind: "edit",
    doing: `${doing} ${what}`,
    done: `${status === "declined" ? "Rejected" : done} ${what}`,
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

/** "+7 −2", uncoloured until the row is hovered; a dot for a created or deleted file. */
function FileCounts({ file }: { file: FileStat }) {
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

/** One file's diff, read from its call: the change its CLI applied, with its lines numbered. */
function FileDiff({ conversationId, requestId, kind, path }: { conversationId: string; requestId: string | null; kind: ToolStep; path: string }) {
  const { items, error } = useLeadItems(conversationId, requestId, kind);
  if (error && !items) return <p className="text-destructive text-xs">{error}</p>;
  if (!items) return <span className="shimmer text-xs">Loading</span>;
  const item = leadAction(items, kind);
  const changes = item?.kind === "files" ? item.changes.filter((change) => (change.movedTo ?? change.path) === path && change.diff) : [];
  if (changes.length === 0) return <InlineDiff path={path} diff={null} />;
  return <>{changes.map((change, index) => <InlineDiff key={index} path={path} diff={change.diff} />)}</>;
}

/**
 * A file change as rows, one per file, each opening to its diff (`diffs`, by path) once it is
 * done: the same rows for the thread's own change and a worker's.
 */
export function FileChangeRows({ files, status, startedAtMs, diffs }: {
  files: readonly FileStat[];
  status: ItemStatus;
  startedAtMs?: number | undefined;
  diffs: ReadonlyMap<string, ReactNode>;
}) {
  return (
    <>
      {files.map((file) => {
        const words = fileWords(file, status);
        return (
          <StepRow
            key={file.path}
            slot="edited-file"
            words={words}
            status={status}
            startedAtMs={startedAtMs}
            objectNode={<FileLink path={file.path} name={words.object ?? basename(file.path)} />}
            trail={status === "inProgress" ? undefined : <FileCounts file={file} />}
            detail={status === "inProgress" ? undefined : diffs.get(file.path)}
          />
        );
      })}
    </>
  );
}

/** A lead's file change, one row per file, each diff read with the request's calls. */
export function EditedFileRows({ step, files, conversationId }: { step: LeadStep; files: readonly FileStat[]; conversationId: string | null }) {
  const { kind } = step;
  if (kind.type !== "tool") return null;
  return (
    <FileChangeRows
      files={files}
      status={kind.status}
      startedAtMs={step.atMs}
      diffs={new Map(
        conversationId && kind.logSeq !== undefined
          ? files.map((file) => [file.path, <FileDiff key={file.path} conversationId={conversationId} requestId={step.requestId ?? null} kind={kind} path={file.path} />])
          : [],
      )}
    />
  );
}

/**
 * A worker's file change's files, counted as the thread's own are: its CLI's counts, else its
 * diff's; a moved file named where it went, with where it was.
 */
export function changeStats(changes: readonly FileChange[]): FileStat[] {
  const byPath = new Map<string, FileStat>();
  for (const change of changes) {
    const counted = change.added !== undefined && change.removed !== undefined
      ? { added: change.added, removed: change.removed }
      : change.diff
        ? diffCounts(parseDiff(change.diff).rows)
        : { added: 0, removed: 0 };
    const path = change.movedTo ?? change.path;
    const seen = byPath.get(path);
    const from = seen?.from ?? (change.movedTo ? change.path : undefined);
    byPath.set(path, {
      path,
      insertions: (seen?.insertions ?? 0) + counted.added,
      deletions: (seen?.deletions ?? 0) + counted.removed,
      binary: false,
      change: seen?.change ?? change.kind,
      ...(from === undefined ? {} : { from }),
    });
  }
  return [...byPath.values()];
}
