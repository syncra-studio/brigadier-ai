import {
  ArrowRotateCcw,
  ArrowRotateCw,
  ArrowUpRight,
  ChevronRightAlt,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { isFinal } from "@/app/conversation/blocks";
import { mergeStats } from "@/app/conversation/ComposerCapsule";
import { undoToast } from "@/app/conversation/undoWords";
import { useViewConversation } from "@/app/conversation/viewContext";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import type { DiffStat, FileStat, HeldPath, ReviewDiff } from "@/ipc/generated";
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

/** A Review diff's files and counts, as a diff stat. */
function statOf(review: ReviewDiff): DiffStat {
  return {
    files: review.files.map(({ path, insertions, deletions, binary }) => ({ path, insertions, deletions, binary })),
    insertions: review.insertions,
    deletions: review.deletions,
  };
}

/**
 * The turn's changes as the Review diff tells them, request by request, summed: fetched when
 * `wanted` (and again once the turn's landed work changes). Its patch gives every file row its
 * own diff; its counts stand in for workers' landings that carry none.
 */
function useTurnReview(
  conversationId: string,
  requests: string,
  wanted: boolean,
  change: string,
) {
  const [state, setState] = useState<{
    change: string;
    stat: DiffStat | null;
    /** Each request's patch: one file changed by two requests has a section in each, never joined. */
    patches: string[] | null;
    error: boolean;
  } | null>(null);
  const key = `${requests}|${change}`;
  const due = wanted && requests !== "" && state?.change !== key;
  useEffect(() => {
    if (!due) return;
    let live = true;
    Promise.all(
      requests.split(" ").map((requestId) =>
        request({
          method: "getReviewDiff",
          conversationId,
          scope: { type: "lastTurn", requestId },
          wholeFiles: false,
          ignoreWhitespace: false,
        }),
      ),
    )
      .then((answers) => {
        if (!live) return;
        const reviews = answers.map(({ review }) => review);
        const stat = mergeStats(reviews.map(statOf));
        setState({
          change: key,
          stat: stat && stat.files.length > 0 ? stat : null,
          patches: reviews.map((review) => review.patch),
          error: false,
        });
      })
      .catch(() => live && setState({ change: key, stat: null, patches: null, error: true }));
    return () => {
      live = false;
    };
  }, [due, conversationId, requests, key]);
  return state?.change === key ? state : null;
}

/** One changed file of the turn: a row that opens to its diff; a binary file or one too long only lists. */
const FileRow: FC<{
  file: FileStat;
  open: boolean;
  onToggle: () => void;
  patch: { patches: string[] | null; error: boolean } | null;
}> = ({ file, open, onToggle, patch }) => {
  const tooLarge = file.insertions + file.deletions > TOO_LARGE_LINES;
  const fixed = file.binary || tooLarge;
  // The file's change in each request that made one, each against its own base: read only for an
  // opened row, as reading a patch goes through all of it.
  const patches = open && !fixed ? (patch?.patches ?? null) : null;
  const sections = useMemo(
    () => patches?.map((each) => filePatch(each, file.path)).filter((section) => section !== "") ?? [],
    [patches, file.path],
  );
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
          ) : patch.error || patch.patches === null ? (
            <p className="text-muted-foreground py-1 text-sm">
              Could not load the changes
            </p>
          ) : sections.length === 0 ? (
            <InlineDiff path={file.path} diff={null} />
          ) : (
            sections.map((section, index) => <InlineDiff key={index} path={file.path} diff={section} />)
          )}
        </div>
      )}
    </li>
  );
};

/**
 * The card under a finished turn's answer: "Edited 3 files" (or the one file's name) over
 * "+a −d", which says "View changes ↗" while hovered, then Undo (where one request's changes can
 * be taken back whole), which turns into Reapply, and View changes. With more than one file it
 * lists them, the first three, each opening to its diff. It sums what each of the turn's requests
 * recorded of its own changes (the thread's own edits and commands and its workers' landings,
 * never what the user changed meanwhile), and for a request from before that was kept, what its
 * workers landed; a message steered in takes over the worker it speaks to. A turn that changed
 * nothing has no card. A landing with no stats of its own (a worker that landed straight onto
 * the branch) is counted from the Review diff.
 */
export const TurnDiff: FC<{ requestIds: readonly string[] }> = ({ requestIds }) => {
  const conversationId = useViewConversation()?.id ?? null;
  // What each request recorded of its own changes.
  const recorded = useBoard(useShallow((s) => requestIds.map((id) => s.board?.requests[id]?.changes ?? null)));
  const hasRecord = (id: string) => {
    const changes = recorded[requestIds.indexOf(id)];
    return !!changes && changes.stat.files.length > 0;
  };
  // Workers' landings, for the requests that recorded nothing (from before it was kept).
  const landed = useBoard(
    useShallow((s) =>
      Object.values(s.board?.tasks ?? {})
        .filter((task) => task.requestId !== null && requestIds.includes(task.requestId) && task.landed !== null)
        .filter((task) => !s.board?.requests[task.requestId ?? ""]?.changes)
        .toSorted((a, b) => a.number - b.number),
    ),
  );
  // The requests that changed something, in the turn's order.
  const requests = requestIds.filter((id) => hasRecord(id) || landed.some((task) => task.requestId === id));
  const reverts = useBoard(
    useShallow((s) => requests.map((id) => s.board?.requests[id]?.undo?.reverted ?? false)),
  );
  const reverted = reverts.length > 0 && reverts.every(Boolean);
  // Undo takes back one request's changes whole: a turn of several requests (whose undos would
  // clash on a file both changed) has none, nor an old landing straight onto the branch (no candidate).
  const undoable = requests.length === 1 && (requests.every(hasRecord) || landed.every((task) => task.candidate !== null));
  // What Undo leaves as it is, and why.
  const held = requests.flatMap((id) => recorded[requestIds.indexOf(id)]?.held ?? []);
  // Undo waits for the turn's work: a request still at work, or a worker that may land more.
  const working = useBoard((s) =>
    requests.some((id) => s.board?.requests[id]?.state.type === "working") ||
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId !== null && requestIds.includes(task.requestId) && !isFinal(task),
    ),
  );
  const [busy, setBusy] = useState(false);
  const [all, setAll] = useState(false);
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const ownStats = requests.flatMap((id) => {
    const changes = recorded[requestIds.indexOf(id)];
    return changes && hasRecord(id) ? [changes.stat] : [];
  });
  const landedKnown = landed.every((task) => task.candidate);
  const known = landedKnown
    ? mergeStats([...ownStats, ...landed.flatMap((task) => (task.candidate ? [task.candidate.diffStat] : []))])
    : null;
  const fetched = useTurnReview(
    conversationId ?? "",
    requests.join(" "),
    (known === null || opened.size > 0) && !!conversationId,
    `${landed.map((task) => `${task.id}:${task.landed}`).join(" ")}|${ownStats.map((stat) => `${stat.files.length}:${stat.insertions}:${stat.deletions}`).join(" ")}`,
  );
  const diff = known ?? fetched?.stat ?? null;
  const [first] = requests;
  if (!diff || !conversationId || first === undefined) return null;

  // The Changes tab shows one request: the turn's first that landed.
  const review = () => {
    setReviewScope(conversationId, { type: "lastTurn", requestId: first });
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
    // Undo goes newest first; Reapply oldest first.
    const due = requests.filter((_, index) => reverts[index] === reverted);
    (reverted ? due : due.toReversed())
      .reduce<Promise<{ restored: number; left: HeldPath[] }>>(
        (before, requestId) =>
          before.then(async (done) => {
            const { outcome } = await request({ method: "undoChanges", conversationId, requestId, reapply: reverted });
            return { restored: done.restored + outcome.restored.length, left: [...done.left, ...outcome.held] };
          }),
        Promise.resolve({ restored: 0, left: [] }),
      )
      .then(({ restored, left }) => toast(undoToast(left, restored, reverted)))
      .catch((error: unknown) =>
        toast(
          `${reverted ? "Couldn't reapply" : "Couldn't undo"} the changes: ${error instanceof Error ? error.message : String(error)}`,
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
        {undoable && <Button
          variant="ghost"
          size="sm"
          data-held={held.length || undefined}
          title={held.length === 0 ? undefined : held.length === 1 ? held[0]?.reason : `Undo leaves ${held.length} files as they are: changed by you or by a command during the request`}
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
        </Button>}
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
              patch={fetched}
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
