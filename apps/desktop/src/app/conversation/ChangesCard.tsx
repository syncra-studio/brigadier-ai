import { type FC, type ReactNode, useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { useAskRequest } from "@/app/conversation/activity/askRequest";
import { mergeWarning } from "@/app/conversation/activity/mergeChecks";
import { mergeViewTarget, mergeWords, type TurnMerge } from "@/app/conversation/activity/MergeMessage";
import { type Merged, redoRequest, revertRequest } from "@/app/conversation/activity/revert";
import { isFinal } from "@/app/conversation/blocks";
import { FoldBody, FoldHeader } from "@/app/conversation/cardFold";
import { mergeStats } from "@/app/conversation/ComposerCapsule";
import { HELD_WHY, undoToast } from "@/app/conversation/undoWords";
import { useViewConversation } from "@/app/conversation/viewContext";
import { LinkButton, Pill, PrimaryPill, QuietPill, RoundBadge, RowText, StorageCard } from "@/app/settings/storage/parts";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import {
  ArrowRotateCcw,
  ArrowRotateCw,
  Branch,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  FileCode,
  PullRequestMerged,
  TriangleExclamationErrorWarning,
  Trash,
} from "@/components/icons";
import type { DiffStat, FileReview, FileStat, HeldPath, MergeUndo, ReviewDiff } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { formatTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useMergeUndo } from "@/state/mergeUndo";
import { keepChanges, undoChanges } from "@/state/pendingChanges";
import { openFileTab, openPanelTab, openReviewTab } from "@/state/sessionTabs";
import { useSettledTabs } from "@/state/tabStates";
import { toast } from "@/state/toasts";

/**
 * The Changes card, last under a finished turn's answer (the user's design, `Of3wE`): a round
 * badge, "Edited 6 files" over "+88 −7 · Merged into main" (or "On <branch>"), the actions for
 * where the work stands, and a chevron that folds the file list away. Merged: Revert, which asks
 * in the card's footer first, and View changes. Not merged yet: Discard (the turn's Undo) and
 * Merge into main. Reverted: one line, "Reverted 6 files", folded. Files that wait for review
 * keep Undo All, Keep All and Review Changes. Each file row opens that file's diff in a tab; the
 * first three show, the rest under "Show N more files". Something really wrong with the merged
 * code (an open review finding, a failed check) is the card's footer row, or, folded, a line in
 * its header: it never folds away.
 */

/** The design's borderless pill: secondary words, 13 like the card's other pills. */
const QUIET = "text-foreground/65 hover:text-foreground text-label";
/** Its white pill, 13 too. */
const PRIMARY = "text-label";

/** The files listed before "Show {n} more files". */
const FIRST_FILES = 3;

/** "+88 −7" in the mono face: green and red, a zero dim; all dim once reverted. */
const Counts: FC<{ insertions: number; deletions: number; dim?: boolean }> = ({ insertions, deletions, dim = false }) => (
  <span data-slot="turn-diff-counts" className="flex shrink-0 items-center gap-2 font-mono text-xs tabular-nums">
    <span className={dim || insertions === 0 ? "text-muted-foreground" : "text-success"}>+{insertions}</span>{" "}
    <span className={dim || deletions === 0 ? "text-muted-foreground" : "text-destructive"}>−{deletions}</span>
  </span>
);

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
 * `wanted` (and again once the turn's landed work changes). Its counts stand in for workers'
 * landings that carry none.
 */
function useTurnReview(conversationId: string, requests: string, wanted: boolean, change: string) {
  const [state, setState] = useState<{ change: string; stat: DiffStat | null } | null>(null);
  const key = `${requests}|${change}`;
  const due = wanted && requests !== "" && state?.change !== key;
  useEffect(() => {
    if (!due) return;
    let live = true;
    Promise.all(
      requests.split(" ").map((requestId) =>
        request({ method: "getReviewDiff", conversationId, scope: { type: "lastTurn", requestId }, wholeFiles: false, ignoreWhitespace: false }),
      ),
    )
      .then((answers) => {
        if (!live) return;
        const stat = mergeStats(answers.map(({ review }) => statOf(review)));
        setState({ change: key, stat: stat && stat.files.length > 0 ? stat : null });
      })
      .catch(() => live && setState({ change: key, stat: null }));
    return () => {
      live = false;
    };
  }, [due, conversationId, requests, key]);
  return state?.change === key ? state : null;
}

/** Where the review left a file, as its row says it; a merge says so once, in the card's header. */
const REVIEWED: Record<Exclude<FileReview, "pending">, string | null> = { later: "Changed later", kept: "Kept", undone: "Undone", merged: null };

/**
 * One changed file: its icon, the folder dim and the name in full, its counts in two columns and
 * a chevron. A click opens its diff in a tab (a file that waits for review, its review). While it
 * waits, Undo and Keep show on hover; once reviewed, where the review left it.
 */
const FileRow: FC<{
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

/**
 * Whether a request's review holds files: one it put back as it was after a merge took it waits
 * from the merged version, though its own net diff shows no change (#333).
 */
const reviewsFiles = (review: Record<string, FileReview> | undefined) => Object.keys(review ?? {}).length > 0;

/**
 * Whether the turn's requests changed code: a change of their own recorded, or a worker of theirs
 * landed. A Chat's never do. An answer that changed nothing has no Details (Q79).
 */
export function useTurnChanged(requestIds: readonly string[]): boolean {
  return useBoard(
    (s) =>
      requestIds.some((id) => (s.board?.requests[id]?.changes?.stat.files.length ?? 0) > 0 || reviewsFiles(s.board?.requests[id]?.review)) ||
      Object.values(s.board?.tasks ?? {}).some(
        (task) => task.requestId !== null && requestIds.includes(task.requestId) && task.landed !== null,
      ),
  );
}

/**
 * Whether the merge was taken back on its base: git shows its revert, not redone since, however
 * it was made (through the thread, whose revert lands as a merge of its own, or by hand). Until
 * git has answered, the session reading Reverted with this as its latest merge. With what git
 * shows of that revert: its time, and the commit Redo reverts.
 */
function useReverted(merge: Merged | null): { reverted: boolean; undo: MergeUndo | null } {
  const conversationId = useViewConversation()?.id ?? "";
  const settled = useSettledTabs((s) => s.byTab[conversationId]?.state === "reverted");
  const latest = useBoard((s) => s.board?.orchestratorSteps.findLast((step) => step.kind.type === "merged")?.kind as Merged | undefined);
  const undo = useMergeUndo(conversationId, merge?.mergeCommit);
  const reverted = !!merge && (undo !== null ? undo.redoneBy === undefined : settled && latest?.mergeCommit === merge.mergeCommit);
  return { reverted, undo: reverted ? undo : null };
}

/**
 * The open review findings of a merge's reviews, by their ids ("R4.1"), with the first one's
 * title: the footer row names it.
 */
function useOpenFinding(merge: Merged | null): { id: string; title: string } | null {
  const tips = merge?.record?.reviews.filter((review) => review.open > 0).map((review) => review.to) ?? [];
  const key = tips.join(" ");
  const found = useBoard((s) => {
    if (!key) return null;
    const runs = Object.values(s.board?.reviews ?? {}).filter((run) => key.split(" ").some((tip) => run.tip.startsWith(tip) || tip.startsWith(run.tip)));
    const item = runs.toSorted((a, b) => a.number - b.number).flatMap((run) => run.items).find((each) => each.status.type === "open");
    return item ? `${item.id}\n${item.title}` : null;
  });
  if (!found) return null;
  const [id = "", title = ""] = found.split("\n");
  return { id, title };
}

/**
 * The Changes card under a finished turn's answer. It sums what each of the turn's requests
 * recorded of its own changes (the thread's own edits and commands and its workers' landings,
 * never what the user changed meanwhile), and for a request from before that was kept, what its
 * workers landed; a message steered in takes over the worker it speaks to. A turn that changed
 * nothing has no card, unless it merged: then the card is the merge's. A landing with no stats
 * of its own (a worker that landed straight onto the branch) is counted from the Review diff.
 */
export const ChangesCard: FC<{ requestIds: readonly string[]; merge?: TurnMerge | null }> = ({ requestIds, merge = null }) => {
  const conversation = useViewConversation();
  const conversationId = conversation?.id ?? null;
  // What each request recorded of its own changes.
  const recorded = useBoard(useShallow((s) => requestIds.map((id) => s.board?.requests[id]?.changes ?? null)));
  // Where the review left each request's files.
  const ownReviews = useBoard(useShallow((s) => requestIds.map((id) => s.board?.requests[id]?.review)));
  const hasRecord = (id: string) => {
    const changes = recorded[requestIds.indexOf(id)];
    return !!changes && (changes.stat.files.length > 0 || reviewsFiles(ownReviews[requestIds.indexOf(id)]));
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
  const undone = useBoard(useShallow((s) => requests.map((id) => s.board?.requests[id]?.undo?.reverted ?? false)));
  // Where the review left each file, the turn's latest request that changed it deciding.
  const reviews = requests.map((id) => ownReviews[requestIds.indexOf(id)]);
  const review: Record<string, FileReview> = Object.assign({}, ...reviews.map((each) => each ?? {}));
  const reviewing = Object.keys(review).length > 0;
  const discarded = undone.length > 0 && undone.every(Boolean);
  // Discard takes back one request's changes whole: a turn of several requests (whose undos would
  // clash on a file both changed) has none, nor an old landing straight onto the branch (no candidate).
  const undoable = requests.length === 1 && (requests.every(hasRecord) || landed.every((task) => task.candidate !== null));
  // What Discard leaves as it is, and why.
  const held = requests.flatMap((id) => recorded[requestIds.indexOf(id)]?.held ?? []);
  // Discard and Merge wait for the turn's work: a request still at work, or a worker that may land more.
  const working = useBoard((s) =>
    requests.some((id) => s.board?.requests[id]?.state.type === "working") ||
    Object.values(s.board?.tasks ?? {}).some((task) => task.requestId !== null && requestIds.includes(task.requestId) && !isFinal(task)),
  );
  const threadBusy = useBoard((s) => s.board?.run === "running" || s.board?.run === "starting");
  const [busy, setBusy] = useState(false);
  const [all, setAll] = useState(false);
  const [confirming, setConfirming] = useState(false);
  // A revert sent from here reads "Reverting…" until it lands, or until the thread's run that
  // took it ends without it.
  const [sent, setSent] = useState<"no" | "sent" | "taken">("no");
  const ownStats = requests.flatMap((id) => {
    const changes = recorded[requestIds.indexOf(id)];
    return changes && hasRecord(id) ? [changes.stat] : [];
  });
  const landedKnown = landed.every((task) => task.candidate);
  const known = landedKnown ? mergeStats([...ownStats, ...landed.flatMap((task) => (task.candidate ? [task.candidate.diffStat] : []))]) : null;
  const fetched = useTurnReview(
    conversationId ?? "",
    requests.join(" "),
    known === null && !!conversationId,
    `${landed.map((task) => `${task.id}:${task.landed}`).join(" ")}|${ownStats.map((stat) => `${stat.files.length}:${stat.insertions}:${stat.deletions}`).join(" ")}`,
  );
  const diff = known ?? fetched?.stat ?? null;
  const merged = merge?.merge ?? null;
  const { reverted, undo } = useReverted(merged);
  const finding = useOpenFinding(merged);
  // Folded by the user, else folded once reverted.
  const [fold, setFold] = useState<{ reverted: boolean; open: boolean } | null>(null);
  const open = fold && fold.reverted === reverted ? fold.open : !reverted;
  const revertText = merged ? revertRequest(merged) : null;
  const revert = useAskRequest(conversationId ?? "", revertText);
  const redoText = merged && reverted ? redoRequest(merged, undo) : null;
  const redo = useAskRequest(conversationId ?? "", redoText);
  const environment = conversation?.setup?.type === "session" ? conversation.setup.environment : null;
  // The session branch's commits its base lacks, as git counted them last.
  const ahead = useSettledTabs((s) => (conversationId ? s.byTab[conversationId]?.ahead : undefined));
  const mergeText = environment?.type === "newWorktree" ? `Merge ${environment.branch} into ${environment.base}` : null;
  const mergeAsk = useAskRequest(conversationId ?? "", mergeText);

  // A turn that merged with nothing of its own counted still shows its merge.
  if (!conversationId || ((!diff || requests.length === 0) && !merge?.own)) return null;

  // A file the review has that the requests' net diff leaves out (put back as it was after a
  // merge) still gets its row (#333).
  const files: FileStat[] = diff
    ? [
        ...diff.files,
        ...Object.keys(review)
          .filter((path) => !diff.files.some((file) => file.path === path))
          .map((path) => ({ path, insertions: 0, deletions: 0, binary: false })),
      ]
    : [];
  const counts = diff ?? { insertions: 0, deletions: 0 };
  const named = files.length === 1 ? (files[0]!.path.split("/").pop() ?? files[0]!.path) : `${files.length} files`;
  const waiting = files.filter((file) => review[file.path] === "pending").map((file) => file.path);
  const waits = reviewing && waiting.length > 0;
  const title = reverted
    ? files.length > 0 ? `Reverted ${named}` : "Reverted the merge"
    : waits
      ? `${files.length} ${files.length === 1 ? "File" : "Files"}`
      : files.length > 0
        ? `Edited ${named}`
        : merged
          ? mergeWords(merged)
          : "";
  const tally = (state: FileReview) => Object.values(review).filter((each) => each === state).length;
  const reviewed = [tally("kept") > 0 && `kept ${tally("kept")}`, tally("undone") > 0 && `undid ${tally("undone")}`].filter(Boolean).join(", ");
  const status = reverted
    ? [`Undone on ${merged?.base ?? "main"}`, undo ? formatTime(undo.revertedAtMs) : null].filter(Boolean).join(" · ")
    : waits
      ? `${waiting.length} to review`
      : [
          reviewed || null,
          merged
            ? files.length > 0
              ? mergeWords(merged)
              : `${merged.commits === 1 ? "1 commit" : `${merged.commits} commits`} from ${merged.branch}`
            : environment
              ? [`On ${environment.branch}`, ahead ? `${ahead} ${ahead === 1 ? "commit" : "commits"} ahead` : null].filter(Boolean).join(" · ")
              : null,
        ]
          .filter(Boolean)
          .join(" · ");
  // Something really wrong with the merged code: the footer row, or folded, the header's line.
  const problem = merged?.record && !reverted ? mergeWarning(merged.record) : null;
  const openFindings = merged?.record?.reviews.reduce((sum, each) => sum + each.open, 0) ?? 0;
  if (sent === "sent" && threadBusy) setSent("taken");
  if (sent === "taken" && !threadBusy) setSent("no");
  const reverting = sent !== "no" && !reverted;
  const shown = all ? files : files.slice(0, FIRST_FILES);
  const more = files.length - FIRST_FILES;
  const lastTouching = (path: string) =>
    requests.findLast((id) => recorded[requestIds.indexOf(id)]?.stat.files.some((file) => file.path === path)) ?? requests.at(-1) ?? null;
  const openDiff = (path: string) => {
    if (review[path] === "pending") return openFileTab(conversationId, path);
    if (merged?.mergeCommit) return openReviewTab(conversationId, { ...mergeViewTarget(merged), path });
    openReviewTab(conversationId, { type: "lastTurn", requestId: lastTouching(path), path, staged: false });
  };
  // Undo All and Keep All take this card's files that wait, one after another; one refused
  // (its toast says why) leaves the rest to go on.
  const each = (what: "keep" | "undo", paths: readonly string[]) => {
    setBusy(true);
    void paths
      .reduce<Promise<unknown>>(
        (before, path) => before.then(() => (what === "keep" ? keepChanges(conversationId, path) : undoChanges(conversationId, path))),
        Promise.resolve(),
      )
      .finally(() => setBusy(false));
  };
  // Discard (the turn's Undo) and, once discarded, Reapply.
  const toggle = () => {
    setBusy(true);
    // Undo goes newest first; Reapply oldest first.
    const due = requests.filter((_, index) => undone[index] === discarded);
    (discarded ? due : due.toReversed())
      .reduce<Promise<{ restored: number; left: HeldPath[] }>>(
        (before, requestId) =>
          before.then(async (done) => {
            const { outcome } = await request({ method: "undoChanges", conversationId, requestId, reapply: discarded });
            return { restored: done.restored + outcome.restored.length, left: [...done.left, ...outcome.held] };
          }),
        Promise.resolve({ restored: 0, left: [] }),
      )
      .then(({ restored, left }) => toast(undoToast(left, restored, discarded)))
      .catch((error: unknown) =>
        toast(`${discarded ? "Couldn't reapply" : "Couldn't undo"} the changes: ${error instanceof Error ? error.message : String(error)}`, { tone: "error" }),
      )
      .finally(() => setBusy(false));
  };

  let actions: ReactNode;
  if (waits) {
    actions = (
      <>
        <QuietPill className={QUIET} disabled={busy} aria-busy={busy || undefined} onClick={() => each("undo", waiting)}>Undo All</QuietPill>
        <PrimaryPill className={PRIMARY} disabled={busy} onClick={() => each("keep", waiting)}>Keep All</PrimaryPill>
        <Pill onClick={() => openFileTab(conversationId, waiting[0]!)}>Review Changes</Pill>
      </>
    );
  } else if (merged && reverted) {
    // Redo asks the thread to revert the revert; "Redoing…" while its run has it.
    actions =
      redoText && redo.asked && threadBusy ? (
        <span role="status" data-slot="merge-redoing" className="text-muted-foreground flex shrink-0 items-center gap-1.5 px-3 text-label">
          <Spinner className="size-icon-sm animate-spin" />
          Redoing…
        </span>
      ) : redoText ? (
        <QuietPill className={QUIET} data-slot="merge-redo" title={redoText} disabled={redo.asked} onClick={redo.ask}>
          <ArrowRotateCw aria-hidden className="size-icon-sm" />
          Redo
        </QuietPill>
      ) : null;
  } else if (merged) {
    actions = (
      <>
        {revertText && !confirming && !reverting && (
          <QuietPill
            className={QUIET}
            data-slot="merge-revert"
            title={revertText}
            onClick={() => {
              setConfirming(true);
              // Folded, the card opens to its question.
              if (!open) setFold({ reverted, open: true });
            }}
          >
            <ArrowRotateCcw aria-hidden className="size-icon-sm" />
            Revert
          </QuietPill>
        )}
        {merged.mergeCommit && (
          <Pill data-slot="merge-view-changes" onClick={() => openReviewTab(conversationId, mergeViewTarget(merged))}>
            View changes
          </Pill>
        )}
      </>
    );
  } else {
    actions = (
      <>
        {undoable && !reviewing && (
          <QuietPill className={QUIET}
            data-slot="turn-diff-undo"
            data-held={held.length || undefined}
            title={held.length === 0 ? undefined : held.length === 1 ? held[0]?.reason : `Discard leaves ${held.length} files as they are: ${HELD_WHY}`}
            disabled={busy || working}
            aria-busy={busy || undefined}
            onClick={toggle}
          >
            {busy ? <Spinner className="size-icon-sm animate-spin" /> : discarded ? <ArrowRotateCw aria-hidden className="size-icon-sm" /> : <Trash aria-hidden className="size-icon-sm" />}
            {discarded ? "Reapply" : "Discard"}
          </QuietPill>
        )}
        {mergeText && !discarded && (
          <PrimaryPill className={PRIMARY} data-slot="turn-diff-merge" title={mergeText} disabled={mergeAsk.asked || working} onClick={mergeAsk.ask}>
            Merge into {environment?.type === "newWorktree" ? environment.base : "main"}
          </PrimaryPill>
        )}
      </>
    );
  }
  const foldable = files.length > 0 || !!problem || confirming || reverting;

  return (
    <StorageCard data-slot="turn-diff" data-state={reverted ? "reverted" : merged ? "merged" : "branch"} data-open={open || undefined}>
      <FoldHeader foldable={foldable} onToggle={() => setFold({ reverted, open: !open })} className="flex items-center gap-4 py-4">
        <RoundBadge>
          {reverted ? (
            <ArrowRotateCcw className="text-foreground/65" />
          ) : merged ? (
            <PullRequestMerged />
          ) : (
            <Branch />
          )}
        </RoundBadge>
        <RowText
          title={<span data-slot="turn-diff-title">{title}</span>}
          line={
            <span className="flex min-w-0 items-center gap-2">
              {files.length > 0 && <Counts insertions={counts.insertions} deletions={counts.deletions} dim={reverted} />}
              {files.length > 0 && status && <span aria-hidden className="text-muted-foreground">·</span>}
              {status && (
                <span data-slot="turn-diff-status" className={cn("min-w-0 truncate", reverted && "text-muted-foreground")}>
                  {status}
                </span>
              )}
              {!open && problem && (
                <>
                  <span aria-hidden className="text-muted-foreground">·</span>
                  <span data-slot="turn-diff-problem" className="text-warning flex shrink-0 items-center gap-1.25">
                    <TriangleExclamationErrorWarning aria-hidden className="size-3" />
                    {openFindings > 0 ? `${openFindings} open ${openFindings === 1 ? "finding" : "findings"}` : problem.replace(/\.$/, "")}
                  </span>
                </>
              )}
            </span>
          }
        />
        <div className="flex shrink-0 items-center gap-1">
          {actions}
          {foldable && (
            <button
              type="button"
              data-slot="turn-diff-fold"
              aria-expanded={open}
              aria-label={open ? "Fold the files" : "Show the files"}
              onClick={() => setFold({ reverted, open: !open })}
              className="text-muted-foreground hover:text-foreground flex size-7.5 items-center justify-center"
            >
              {open ? <ChevronUp aria-hidden className="size-3.75" /> : <ChevronDown aria-hidden className="size-3.75" />}
            </button>
          )}
        </div>
      </FoldHeader>
      <FoldBody open={open}>
        {files.length > 0 && (
          <ul data-slot="turn-diff-files" className="flex flex-col ps-13">
            {shown.map((file) => (
              <FileRow
                key={file.path}
                file={file}
                review={review[file.path]}
                busy={busy}
                onOpen={() => openDiff(file.path)}
                onAct={(what) => each(what, [file.path])}
              />
            ))}
            {more > 0 && (
              <li className="border-divider border-t pt-2.5 pb-3">
                <LinkButton data-slot="turn-diff-more" icon={all ? ChevronUp : ChevronDown} className="text-label" onClick={() => setAll((now) => !now)}>
                  {all ? "Show less" : `Show ${more} more ${more === 1 ? "file" : "files"}`}
                </LinkButton>
              </li>
            )}
          </ul>
        )}
        {problem && (
          <div data-slot="merge-warning" className="border-divider flex items-center gap-3 border-t pt-3 pb-3.5 ps-13">
            <TriangleExclamationErrorWarning aria-hidden className="text-warning size-3.5 shrink-0" />
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span data-slot="merge-warning-title" className="text-warning text-label font-medium">{problem.replace(/\.$/, "")}</span>
              {finding && (
                <span data-slot="merge-warning-finding" className="text-muted-foreground truncate text-xs">
                  {finding.id} · {finding.title}
                </span>
              )}
            </div>
            <QuietPill className={QUIET} data-slot="merge-warning-open" onClick={() => openPanelTab(conversationId, { type: "reviews" }, "Progress")}>
              {openFindings > 0 ? "Open finding" : "Open Progress"}
            </QuietPill>
          </div>
        )}
        {merged && (confirming || reverting) && (
          <div data-slot="merge-revert-confirm" className="border-divider flex items-center gap-3 border-t pt-3.5 pb-4 ps-13">
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="text-foreground text-label font-medium">
                {files.length > 0 ? `Revert ${files.length === 1 ? "this file" : `these ${files.length} files`} on ${merged.base}?` : `Revert this merge on ${merged.base}?`}
              </span>
              <span className="text-muted-foreground text-xs">
                {`Adds one commit that undoes ${files.length > 0 ? `+${counts.insertions} −${counts.deletions}` : "it"}${merged.pr === undefined ? "" : " and pushes it"}. Work merged after this stays.`}
              </span>
            </div>
            {reverting ? (
              <span role="status" className="text-muted-foreground flex shrink-0 items-center gap-1.5 text-label">
                <Spinner className="size-icon-sm animate-spin" />
                Reverting…
              </span>
            ) : (
              <>
                <QuietPill className={QUIET} onClick={() => setConfirming(false)}>Cancel</QuietPill>
                <PrimaryPill className={PRIMARY}
                  data-slot="merge-revert-go"
                  onClick={() => {
                    revert.ask();
                    setSent("sent");
                    setConfirming(false);
                  }}
                >
                  {files.length === 0 ? "Revert" : files.length === 1 ? "Revert 1 file" : `Revert ${files.length} files`}
                </PrimaryPill>
              </>
            )}
          </div>
        )}
      </FoldBody>
    </StorageCard>
  );
};
