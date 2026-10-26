import { type FC, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { useAskRequest } from "@/app/conversation/activity/askRequest";
import { mergeWords, type TurnMerge } from "@/app/conversation/activity/MergeMessage";
import { FoldBody, FoldHeader } from "@/app/conversation/cardFold";
import { ChangesFileRow } from "@/app/conversation/ChangesFileRow";
import { PRIMARY, QUIET } from "@/app/conversation/changesPills";
import { mergeStats } from "@/app/conversation/mergeStats";
import { RevertConfirmRow } from "@/app/conversation/RevertConfirmRow";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ArrowRotateCcw, Branch, ChevronDown, ChevronUp, PullRequestMerged } from "@/components/icons";
import { Button, RoundBadge } from "@/components/controls";
import { Counts, RowText, SettingsCard } from "@/components/shared";
import type { FileReview, MergedTurnChanges, TurnRevert } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { keepChanges, undoChanges } from "@/state/pendingChanges";
import { openFileTab, openReviewTab } from "@/state/sessionTabs";
import { nothingToMerge, useSettledTabs } from "@/state/tabStates";
import { toast } from "@/state/toasts";
import { useKeptState } from "@/components/conversation";

const FIRST_FILES = 3;
const FIRST_MERGES = 3;
const mergeKey = (group: MergedTurnChanges) => `${group.mergeCommit}:${group.tip}`;

/** Merge attribution stays compact until its files are requested. */
const MergedChanges: FC<{ group: MergedTurnChanges; conversationId: string; busy: boolean }> = ({ group, conversationId, busy }) => {
  const key = `merge:${mergeKey(group)}`;
  const [open, setOpen] = useKeptState(`${key}:open`, false);
  const [all, setAll] = useKeptState(`${key}:files`, false);
  const workers = useBoard((s) => s.board?.tasks);
  const matches = Object.values(workers ?? {}).filter((task) => task.workspace?.branch === group.branch);
  const worker = matches.length === 1 ? matches[0] : undefined;
  const sha = group.branch === group.tip && /^[a-f0-9]{40,64}$/i.test(group.branch);
  const label = worker?.title ?? (sha ? `Merged changes · ${group.branch.slice(0, 8)}` : group.branch);
  const files = group.stat.files;
  const more = files.length - FIRST_FILES;

  return <div data-slot="turn-merged-branch" data-worker={worker?.id} data-merge={mergeKey(group)} className="ps-13">
    <button type="button" data-slot="turn-merged-fold" aria-expanded={open} onClick={() => setOpen(!open)} className="border-divider hover:text-foreground flex w-full items-center gap-2 border-t py-3 text-start text-label">
      <span data-slot="turn-merged-title" title={group.branch} className="min-w-0 flex-1 truncate">{label}</span>
      <span className="text-muted-foreground shrink-0 text-xs">{files.length} {files.length === 1 ? "file" : "files"}</span>
      <Counts insertions={group.stat.insertions} deletions={group.stat.deletions} />
      {open ? <ChevronUp aria-hidden className="size-3.75 shrink-0" /> : <ChevronDown aria-hidden className="size-3.75 shrink-0" />}
    </button>
    <FoldBody open={open}>
      <ul>{(all ? files : files.slice(0, FIRST_FILES)).map((file) => <ChangesFileRow key={file.path} file={file} review={undefined} busy={busy} onOpen={() => openReviewTab(conversationId, { type: "commit", commit: group.mergeCommit, path: file.path, staged: false })} onAct={() => {}} />)}
        {more > 0 && <li className="border-divider border-t pt-2.5 pb-3"><Button variant="link-blue" size="inline" data-slot="turn-merged-more-files" aria-expanded={all} className="text-label" onClick={() => setAll(!all)}>{all ? "Show less" : `Show ${more} more ${more === 1 ? "file" : "files"}`}{all ? <ChevronUp aria-hidden /> : <ChevronDown aria-hidden />}</Button></li>}
      </ul>
    </FoldBody>
  </div>;
};

/** Only a retained git diff can make a turn's Changes card appear. */
export function useTurnChanged(requestIds: readonly string[]): boolean {
  return useBoard((s) => requestIds.some((id) => {
    const changes = s.board?.requests[id]?.changes;
    return !!changes && (changes.stat.files.length > 0 || (changes.merged?.length ?? 0) > 0);
  }));
}

/** The existing Changes card, fed by turn trees and git's merge attribution. */
export const ChangesCard: FC<{ requestIds: readonly string[]; merge?: TurnMerge | null }> = ({ requestIds, merge = null }) => {
  const conversation = useViewConversation();
  const conversationId = conversation?.id ?? null;
  const records = useBoard(useShallow((s) => requestIds.map((id) => s.board?.requests[id] ?? null)));
  const snapshots = records.filter((record) => record?.changes?.settled);
  const diff = mergeStats(snapshots.flatMap((record) => record?.changes ? [record.changes.stat] : []));
  const groups = [...new Map(snapshots.flatMap((record) => record?.changes?.merged ?? []).map((group) => [mergeKey(group), group])).values()];
  const reverted = snapshots.length > 0 && snapshots.every((record) => record?.undo?.reverted);
  const review: Record<string, FileReview> = Object.assign({}, ...snapshots.map((record) => record?.review ?? {}));
  const working = useBoard((s) => s.board?.run === "running" || s.board?.run === "starting");
  const [busy, setBusy] = useState(false);
  const [all, setAll] = useKeptState("files", false);
  const [allMerges, setAllMerges] = useKeptState("merges", false);
  const [preview, setPreview] = useState<{ requestId: string; revert: TurnRevert } | null>(null);
  const [fold, setFold] = useKeptState<{ reverted: boolean; open: boolean } | null>("fold", null);
  const open = fold?.reverted === reverted ? fold.open : !reverted;
  const environment = conversation?.setup?.type === "session" ? conversation.setup.environment : null;
  const ahead = useSettledTabs((s) => conversationId ? s.byTab[conversationId]?.ahead : undefined);
  // Its work is on the base already (D61): no count and no Merge.
  const landed = useSettledTabs((s) => conversationId ? nothingToMerge(s.byTab[conversationId]) : false);
  const merged = merge?.merge ?? null;
  const mergeText = environment?.type === "newWorktree" ? `Merge ${environment.branch} into ${environment.base}` : null;
  const mergeAsk = useAskRequest(conversationId ?? "", mergeText);

  if (!conversationId || !diff || (diff.files.length === 0 && groups.length === 0)) return null;
  const files = diff.files;
  const waiting = files.filter((file) => review[file.path] === "pending").map((file) => file.path);
  const waits = waiting.length > 0;
  const named = files.length === 1 ? (files[0]!.path.split("/").pop() ?? files[0]!.path) : `${files.length} files`;
  const title = reverted ? `Reverted ${named}` : waits ? `${files.length} ${files.length === 1 ? "File" : "Files"}` : files.length > 0 ? `Edited ${named}` : "Merged changes";
  const tally = (state: FileReview) => Object.values(review).filter((each) => each === state).length;
  const reviewed = [tally("kept") > 0 && `kept ${tally("kept")}`, tally("undone") > 0 && `undid ${tally("undone")}`].filter(Boolean).join(", ");
  const status = reverted ? "Restored to the start of this turn" : waits ? `${waiting.length} to review` : [reviewed || null, merged ? mergeWords(merged, true) : environment ? [`On ${environment.branch}`, landed ? null : ahead ? `${ahead} ${ahead === 1 ? "commit" : "commits"} ahead` : null].filter(Boolean).join(" · ") : null].filter(Boolean).join(" · ");
  const shown = all ? files : files.slice(0, FIRST_FILES);
  const more = files.length - FIRST_FILES;
  const requestFor = (path?: string) => snapshots.findLast((record) => path === undefined || record?.changes?.stat.files.some((file) => file.path === path))?.id ?? null;
  const openDiff = (path?: string) => openReviewTab(conversationId, { type: "lastTurn", requestId: requestFor(path), ...(path !== undefined && { path }), staged: false });
  const each = (what: "keep" | "undo", paths: readonly string[]) => {
    setBusy(true);
    void paths.reduce<Promise<unknown>>((before, path) => before.then(() => what === "keep" ? keepChanges(conversationId, path) : undoChanges(conversationId, path)), Promise.resolve()).finally(() => setBusy(false));
  };
  const beginRevert = () => {
    const requestId = snapshots[0]?.id;
    if (!requestId) return;
    setBusy(true);
    void request({ method: "prepareTurnRevert", conversationId, requestId }).then(({ preview: revert }) => {
      setPreview({ requestId, revert });
      setFold({ reverted, open: true });
    }).catch((error: unknown) => toast(`Couldn't prepare Revert: ${String(error)}`, { tone: "error" })).finally(() => setBusy(false));
  };
  const revert = () => {
    if (!preview) return;
    setBusy(true);
    void request({ method: "revertTurn", conversationId, requestId: preview.requestId, expectedTree: preview.revert.expectedTree }).then(() => setPreview(null)).catch((error: unknown) => {
      setPreview(null);
      toast(`Couldn't revert the changes: ${String(error)}`, { tone: "error" });
    }).finally(() => setBusy(false));
  };
  const foldable = files.length > 0 || groups.length > 0 || preview !== null;

  return (
    <SettingsCard inset split={false} data-slot="turn-diff" data-state={reverted ? "reverted" : merged ? "merged" : "branch"} data-open={open || undefined}>
      <FoldHeader foldable={foldable} onToggle={() => setFold({ reverted, open: !open })} className="flex items-center gap-4 py-4">
        <RoundBadge>{reverted ? <ArrowRotateCcw className="text-foreground/65" /> : merged ? <PullRequestMerged /> : <Branch />}</RoundBadge>
        <RowText title={<span data-slot="turn-diff-title">{title}</span>} line={
          <span className="flex min-w-0 items-center gap-2">
            {files.length > 0 && <Counts data-slot="turn-diff-counts" insertions={diff.insertions} deletions={diff.deletions} dim={reverted} />}
            {files.length > 0 && status && <span aria-hidden className="text-muted-foreground">·</span>}
            {status && <span data-slot="turn-diff-status" className={cn("min-w-0 truncate", reverted && "text-muted-foreground")}>{status}</span>}
          </span>
        } />
        <div className="flex shrink-0 items-center gap-1">
          {waits ? <>
            <Button variant="ghost" size="sm" className={QUIET} disabled={busy} onClick={() => each("undo", waiting)}>Undo All</Button>
            <Button size="sm" className={PRIMARY} disabled={busy} onClick={() => each("keep", waiting)}>Keep All</Button>
            <Button variant="soft" size="sm" onClick={() => openFileTab(conversationId, waiting[0]!)}>Review Changes</Button>
          </> : <>
            {!reverted && !preview && <Button variant="ghost" size="sm" className={QUIET} data-slot="turn-diff-undo" disabled={busy || working} onClick={beginRevert}><ArrowRotateCcw aria-hidden className="size-icon-sm" />Revert</Button>}
            {mergeText && !merged && !reverted && !landed && <Button size="sm" className={PRIMARY} data-slot="turn-diff-merge" title={mergeText} disabled={mergeAsk.asked || working} onClick={mergeAsk.ask}>Merge into {environment?.type === "newWorktree" ? environment.base : "main"}</Button>}
            {merged && <Button variant="soft" size="sm" data-slot="merge-view-changes" onClick={() => openDiff()}>View changes</Button>}
          </>}
          {foldable && <button type="button" data-slot="turn-diff-fold" aria-expanded={open} aria-label={open ? "Fold the files" : "Show the files"} onClick={() => setFold({ reverted, open: !open })} className="text-muted-foreground hover:text-foreground flex size-7.5 items-center justify-center">{open ? <ChevronUp aria-hidden className="size-3.75" /> : <ChevronDown aria-hidden className="size-3.75" />}</button>}
        </div>
      </FoldHeader>
      <FoldBody open={open}>
        {files.length > 0 && <ul data-slot="turn-diff-files" className="flex flex-col ps-13">
          {shown.map((file) => <ChangesFileRow key={file.path} file={file} review={review[file.path]} busy={busy} onOpen={() => openDiff(file.path)} onAct={(what) => each(what, [file.path])} />)}
          {more > 0 && <li className="border-divider border-t pt-2.5 pb-3"><Button variant="link-blue" size="inline" data-slot="turn-diff-more" className="text-label" onClick={() => setAll(!all)}>{all ? "Show less" : `Show ${more} more ${more === 1 ? "file" : "files"}`}{all ? <ChevronUp aria-hidden /> : <ChevronDown aria-hidden />}</Button></li>}
        </ul>}
        {(allMerges ? groups : groups.slice(0, FIRST_MERGES)).map((group) => <MergedChanges key={mergeKey(group)} group={group} conversationId={conversationId} busy={busy} />)}
        {groups.length > FIRST_MERGES && <div className="ps-13"><div className="border-divider border-t pt-2.5 pb-3"><Button variant="link-blue" size="inline" data-slot="turn-merged-more" aria-expanded={allMerges} className="text-label" onClick={() => setAllMerges(!allMerges)}>{allMerges ? "Show fewer merges" : `Show ${groups.length - FIRST_MERGES} more ${groups.length - FIRST_MERGES === 1 ? "merge" : "merges"}`}{allMerges ? <ChevronUp aria-hidden /> : <ChevronDown aria-hidden />}</Button></div></div>}
        {preview && <RevertConfirmRow fileCount={files.length} changedAfter={preview.revert.changedAfter} reverting={busy} onCancel={() => setPreview(null)} onRevert={revert} />}
      </FoldBody>
    </SettingsCard>
  );
};
