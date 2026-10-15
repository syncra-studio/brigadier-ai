import {
  ArrowRotateCcw,
  ArrowRotateCw,
  ArrowUpRight,
  ChevronRightAlt,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useEffect, useState } from "react";

import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { isFinal } from "@/app/conversation/blocks";
import { useRequestDiff } from "@/app/conversation/ComposerCapsule";
import { useViewConversation } from "@/app/conversation/viewContext";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import type { FileStat } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { filePatch } from "@/lib/patch";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { setReviewScope } from "@/state/review";
import { openReviewTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";

/** The files listed before "Show {n} more files". */
const FIRST_FILES = 3;
/** A file changing more lines than this only lists: its diff is not drawn in the thread. */
const TOO_LARGE_LINES = 5000;

const Counts: FC<{
  insertions: number;
  deletions: number;
  className?: string;
}> = ({ insertions, deletions, className }) => (
  <span data-slot="turn-diff-counts" className={cn("tabular-nums", className)}>
    <span className="text-success">+{insertions}</span>{" "}
    <span className="text-destructive">−{deletions}</span>
  </span>
);

/** A path as its rows read: the folder dim, the file's name in full tone. */
const PathWords: FC<{ path: string }> = ({ path }) => {
  const cut = path.lastIndexOf("/") + 1;
  return (
    <span className="min-w-0 flex-1 truncate" title={path}>
      <span data-slot="turn-diff-dir" className="text-foreground/70">
        {path.slice(0, cut)}
      </span>
      <span data-slot="turn-diff-name" className="text-foreground">
        {path.slice(cut)}
      </span>
    </span>
  );
};

/**
 * The turn's patch, fetched the first time a file row opens (and again once the turn's landed
 * work changes): every row's own diff is its section of it.
 */
function useTurnPatch(
  conversationId: string,
  requestId: string,
  wanted: boolean,
  change: string,
) {
  const [state, setState] = useState<{
    change: string;
    patch: string | null;
    error: boolean;
  } | null>(null);
  const due = wanted && state?.change !== change;
  useEffect(() => {
    if (!due) return;
    let live = true;
    request({
      method: "getReviewDiff",
      conversationId,
      scope: { type: "lastTurn", requestId },
      wholeFiles: false,
      ignoreWhitespace: false,
    })
      .then(
        ({ review }) =>
          live && setState({ change, patch: review.patch, error: false }),
      )
      .catch(() => live && setState({ change, patch: null, error: true }));
    return () => {
      live = false;
    };
  }, [due, conversationId, requestId, change]);
  return state?.change === change ? state : null;
}

/** One changed file of the turn: a row that opens to its diff; a binary file or one too long only lists. */
const FileRow: FC<{
  file: FileStat;
  open: boolean;
  onToggle: () => void;
  patch: { patch: string | null; error: boolean } | null;
}> = ({ file, open, onToggle, patch }) => {
  const tooLarge = file.insertions + file.deletions > TOO_LARGE_LINES;
  const fixed = file.binary || tooLarge;
  return (
    <li data-slot="turn-diff-file" data-open={open || undefined}>
      <button
        type="button"
        aria-expanded={fixed ? undefined : open}
        disabled={fixed}
        onClick={onToggle}
        className="group/file hover:bg-foreground/5 flex h-9 w-full items-center gap-3 px-3 text-start text-sm transition-colors disabled:cursor-default disabled:hover:bg-transparent"
      >
        <PathWords path={file.path} />
        {tooLarge && (
          <span className="text-muted-foreground shrink-0 text-xs">
            Too large to render inline
          </span>
        )}
        {file.binary ? (
          <span className="text-muted-foreground text-xs">binary</span>
        ) : (
          <Counts insertions={file.insertions} deletions={file.deletions} />
        )}
      </button>
      {open && !fixed && (
        <div data-slot="turn-diff-file-body" className="px-3 pb-3">
          {patch === null ? (
            <p role="status" className="text-muted-foreground py-1 text-sm">
              Loading changes…
            </p>
          ) : patch.error || patch.patch === null ? (
            <p className="text-muted-foreground py-1 text-sm">
              Could not load the changes
            </p>
          ) : (
            <InlineDiff
              path={file.path}
              diff={filePatch(patch.patch, file.path) || null}
            />
          )}
        </div>
      )}
    </li>
  );
};

/**
 * The card under a finished turn's answer: "Edited 3 files" (or the one file's name) over
 * "+a −d", which says "View changes ↗" while hovered, then Undo, which turns into Reapply, and
 * View changes. With more than one file it lists them, the first three, each opening to its
 * diff. It sums what the request's workers landed, so it keeps its counts in both states; a
 * request that landed nothing has no card.
 */
export const TurnDiff: FC<{ requestId: string }> = ({ requestId }) => {
  const conversationId = useViewConversation()?.id ?? null;
  const diff = useRequestDiff(requestId);
  const reverted = useBoard(
    (s) => s.board?.requests[requestId]?.undo?.reverted ?? false,
  );
  // Undo waits for the request's workers: one still at work may land more.
  const working = useBoard((s) =>
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId === requestId && !isFinal(task),
    ),
  );
  const [busy, setBusy] = useState(false);
  const [all, setAll] = useState(false);
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const change = diff
    ? `${diff.insertions}:${diff.deletions}:${diff.files.length}`
    : "";
  const patch = useTurnPatch(
    conversationId ?? "",
    requestId,
    opened.size > 0 && !!conversationId,
    change,
  );
  if (!diff || !conversationId) return null;

  const review = () => {
    setReviewScope(conversationId, { type: "lastTurn", requestId });
    openReviewTab(conversationId, { type: "all" });
  };

  const [only] = diff.files;
  const one =
    diff.files.length === 1 && only
      ? only.path.split("/").pop() || only.path
      : null;
  const title = one ? `Edited ${one}` : `Edited ${diff.files.length} files`;
  const shown = all ? diff.files : diff.files.slice(0, FIRST_FILES);
  const more = diff.files.length - FIRST_FILES;
  const toggle = () => {
    setBusy(true);
    request({
      method: "undoChanges",
      conversationId,
      requestId,
      reapply: reverted,
    })
      .then(() => toast(reverted ? "Changes reapplied" : "Changes reverted"))
      .catch((error: unknown) =>
        toast(
          `Failed to revert changes: ${error instanceof Error ? error.message : String(error)}`,
          { tone: "error" },
        ),
      )
      .finally(() => setBusy(false));
  };
  const flip = (path: string) =>
    setOpened((now) => {
      const next = new Set(now);
      if (!next.delete(path)) next.add(path);
      return next;
    });

  return (
    <div
      data-slot="turn-diff"
      className="border-foreground/10 rounded-shell overflow-hidden border"
    >
      <div className="group/diff flex items-center gap-3 p-3">
        <button
          type="button"
          aria-label="View changed files"
          onClick={review}
          className="flex min-w-0 flex-1 items-center gap-3 text-start"
        >
          <span className="bg-foreground/5 text-foreground rounded-control flex size-control-lg shrink-0 items-center justify-center">
            <DiffGlyph className="size-icon-md" />
          </span>
          <span className="flex min-w-0 flex-col">
            <span
              data-slot="turn-diff-title"
              className="text-foreground truncate text-sm font-medium"
            >
              {title}
            </span>
            <Counts
              insertions={diff.insertions}
              deletions={diff.deletions}
              className="text-xs group-hover/diff:hidden"
            />
            <span className="text-muted-foreground hidden items-center gap-0.5 text-xs group-hover/diff:flex">
              View changes
              <ArrowUpRight className="size-icon-xs" />
            </span>
          </span>
        </button>
        <Button
          variant="ghost"
          size="sm"
          disabled={busy || working}
          aria-busy={busy || undefined}
          onClick={toggle}
          className="text-foreground font-normal"
        >
          {reverted ? "Reapply" : "Undo"}
          {busy ? (
            <Spinner className="size-icon-sm animate-spin" />
          ) : reverted ? (
            <ArrowRotateCw className="size-icon-sm" />
          ) : (
            <ArrowRotateCcw className="size-icon-sm" />
          )}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={review}
          className="border-border border font-normal"
        >
          View changes
        </Button>
      </div>
      {!one && (
        <ul className="border-border border-t">
          {shown.map((file) => (
            <FileRow
              key={file.path}
              file={file}
              open={opened.has(file.path)}
              onToggle={() => flip(file.path)}
              patch={patch}
            />
          ))}
          {more > 0 && (
            <li>
              <button
                type="button"
                data-slot="turn-diff-more"
                onClick={() => setAll((now) => !now)}
                className="text-muted-foreground hover:text-foreground flex h-9 w-full items-center gap-1 px-3 text-start text-sm transition-colors"
              >
                {all
                  ? "Collapse files"
                  : `Show ${more} more ${more === 1 ? "file" : "files"}`}
                <ChevronRightAlt
                  aria-hidden
                  className={cn(
                    "size-icon-sm transition-transform",
                    all ? "-rotate-90" : "rotate-90",
                  )}
                />
              </button>
            </li>
          )}
        </ul>
      )}
    </div>
  );
};
