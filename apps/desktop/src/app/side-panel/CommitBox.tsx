import { type FC, type RefObject, useEffect, useState } from "react";

import { CommitActionsMenu } from "@/app/side-panel/CommitActionsMenu";
import { type CommitHow, CONFLICTS_LEFT, mainAction, nothingStaged, reason, setStageAll, sourceCalls, type SourceTarget, targetKey, unsent, useStageAll } from "@/app/side-panel/sourceTarget";
import { ArrowDown, ArrowUp, Branch, Check, ChevronDown, PublishGlyph, Sparkles, Spinner, SyncGlyph } from "@/components/icons";
import { TooltipIconButton, Button } from "@/components/controls";
import { Dialog, DialogContent, DialogDescription, DialogTitle, DropdownMenu, DropdownMenuTrigger } from "@/components/overlays";
import { request } from "@/ipc/client";
import type { SourceState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * The message box (blank: written for you; ✨ writes one now; during a merge, git's merge
 * message; during a rebase, the message of the commit it stopped on, kept as it is), the main
 * button with its menu ([`mainAction`] says what it does), and the branch, always in view. With
 * nothing to commit the box and Commit show disabled. Commit with changes but nothing staged
 * asks whether to stage them all, as Cursor does.
 */
export const CommitBox: FC<{
  target: SourceTarget;
  state: SourceState | null;
  /** A stage, unstage, discard or git action is under way. */
  staging: boolean;
  /** The header's ✓ and the More Actions menu commit through it. */
  commitRef: RefObject<((how: CommitHow) => void) | null>;
  onCommitting: (committing: boolean) => void;
  onCommitted: () => void;
  /** Sync Changes, and Commit & Sync's pull and push after the commit. */
  onSync: () => void;
  /** Publish Branch. */
  onPublish: () => void;
  /** Continue during a rebase. */
  onContinueRebase: () => void;
  /** Opens Checkout to…; null where the branch can't change. */
  onBranch: (() => void) | null;
  /** Something is under way: the branch can't change meanwhile. */
  locked: boolean;
  /** Nothing to commit: the box and Commit are off. */
  idle?: boolean;
  /** In a stacked row, which names the branch itself. */
  inRow?: boolean;
}> = ({ target, state, staging, commitRef, onCommitting, onCommitted, onSync, onPublish, onContinueRebase, onBranch, locked, idle = false, inRow = false }) => {
  const key = targetKey(target);
  const [message, setMessageState] = useState(() => unsent.get(key) ?? "");
  const setMessage = (next: string) => {
    if (next) unsent.set(key, next);
    else unsent.delete(key);
    setMessageState(next);
  };
  const [busy, setBusyState] = useState<CommitHow | "write" | null>(null);
  const setBusy = (next: CommitHow | "write" | null) => {
    setBusyState(next);
    onCommitting(next !== null);
  };
  const mac = useApp((s) => s.info?.platform === "macos");
  const merging = state?.merging ?? null;
  const rebasing = state?.rebasing ?? null;
  // A merge starts with git's message in the box, unless the user had written one.
  const [prefilled, setPrefilled] = useState<string | null>(null);
  if (merging && merging.message !== prefilled) {
    setPrefilled(merging.message);
    if (!message) setMessage(merging.message);
  } else if (state && !merging && prefilled !== null) {
    // The merge ended (committed or aborted): its message goes unless the user rewrote it.
    setPrefilled(null);
    if (message === prefilled) setMessage("");
  }
  const conflicted = state?.changes.some((file) => file.status === "conflicted") ?? false;
  const stageAll = useStageAll((s) => s.choice);
  const toStage = nothingStaged(state);
  const action = mainAction(state, stageAll);
  const [asking, setAsking] = useState<CommitHow | null>(null);
  const nothing = !state || (merging || rebasing ? conflicted : state.staged.length === 0 && (!toStage || stageAll === "never"));
  const blocked = (how: CommitHow) =>
    (how === "amend" ? !state || merging !== null : nothing) ||
    ((how === "push" || how === "sync") && !state?.remote) ||
    (rebasing !== null && how !== "commit");
  const commit = (how: CommitHow) => {
    if (blocked(how) || busy || staging) {
      if ((merging || rebasing) && conflicted) toast(CONFLICTS_LEFT, { tone: "error" });
      return;
    }
    if (rebasing) {
      onContinueRebase();
      return;
    }
    // An amend with nothing staged rewrites the last commit's message, staging nothing.
    if (toStage && how !== "amend" && stageAll === "ask") {
      setAsking(how);
      return;
    }
    send(how, toStage && how !== "amend");
  };
  const send = (how: CommitHow, includeUnstaged: boolean) => {
    setBusy(how);
    sourceCalls(target)
      .commit(message.trim() || null, how === "push", how === "amend", includeUnstaged)
      .then(({ outcome }) => {
        const done = outcome.pushed
          ? `Committed and pushed to ${outcome.branch}`
          : `Committed to ${outcome.branch}`;
        // Why the chosen writer model didn't write the message, when it didn't.
        toast(outcome.note ? `${done}. ${outcome.note}` : done);
        setMessage("");
        if (how === "sync") onSync();
      })
      .catch((failure: unknown) => toast(`Failed to commit: ${reason(failure)}`, { tone: "error" }))
      .finally(() => {
        setBusy(null);
        onCommitted();
      });
  };
  useEffect(() => {
    commitRef.current = commit;
  });
  const write = () => {
    setBusy("write");
    request({ method: "writeCommitMessage", target, includeUnstaged: toStage })
      .then(({ message: written }) => setMessage(written))
      .catch((failure: unknown) => toast(`Failed to write a message: ${reason(failure)}`, { tone: "error" }))
      .finally(() => setBusy(null));
  };
  const writing = busy !== null && busy !== "write" && !message.trim() && !merging;
  const label = merging || rebasing ? "Continue" : "Commit";
  const placeholder = `Message (${mac ? "⌘⏎" : "Ctrl+⏎"} to commit on "${state?.branch ?? "…"}")`;
  const off = idle && busy === null;
  const empty = !(rebasing ? rebasing.message : message);
  // With nothing to commit and one checkout, one line: the row's narrower box wraps as a working one does.
  const oneLine = off && empty && !inRow;
  const canWrite = !merging && !rebasing && !!state && (state.staged.length > 0 || (toStage && stageAll !== "never"));
  const mainDisabled = !action.enabled || busy !== null || staging;
  const mainRun = () => {
    if (action.kind === "publish") onPublish();
    else if (action.kind === "sync") onSync();
    else commit("commit");
  };
  const mainWorking = action.kind === "publish" || action.kind === "sync" ? staging : busy === "commit";
  return (
    <div className="flex shrink-0 flex-col gap-2 px-2 pb-1">
      <div className={cn("border-border rounded-control focus-within:border-ring flex items-start border", off && "border-border/60")}>
        {/* An empty box is as tall as its placeholder: WebKit sizes it to one line (#490). */}
        <div className="grid min-w-0 flex-1">
          <span aria-hidden data-slot="commit-message-placeholder" className={cn(
            "col-start-1 row-start-1 max-h-40 overflow-hidden px-2 py-1.5 text-sm",
            oneLine ? "text-muted-foreground pointer-events-none truncate" : "invisible break-words whitespace-pre-wrap",
          )}>
            {empty ? placeholder : ""}
          </span>
          <textarea
            rows={1}
            value={rebasing ? rebasing.message : message}
            disabled={busy !== null || rebasing !== null || off}
            aria-label="Commit message"
            placeholder={oneLine ? "" : placeholder}
            onChange={(event) => setMessage(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (mac ? event.metaKey : event.ctrlKey)) {
                event.preventDefault();
                commit("commit");
              }
            }}
            className={cn(
              "placeholder:text-muted-foreground col-start-1 row-start-1 max-h-40 w-full min-w-0 resize-none bg-transparent px-2 py-1.5 text-sm outline-none",
              oneLine ? "h-full overflow-hidden" : "field-sizing-content",
            )}
          />
        </div>
        <TooltipIconButton
          tooltip="Generate Commit Message"
          size="icon-xs"
          className="m-1"
          disabled={busy !== null || !canWrite}
          onClick={write}
        >
          {busy === "write" ? <Spinner className="size-icon-xs animate-spin motion-reduce:animate-none" /> : <Sparkles />}
        </TooltipIconButton>
      </div>
      <div className="flex">
        <Button
          size="sm"
          data-slot="source-main-button"
          className="min-w-0 flex-1 rounded-e-none"
          disabled={mainDisabled}
          title={(merging || rebasing) && conflicted ? "Resolve the merge conflicts first" : toStage && nothing ? "Stage changes to commit them" : undefined}
          onClick={mainRun}
        >
          {mainWorking ? (
            <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" />
          ) : action.kind === "sync" ? (
            <SyncGlyph />
          ) : action.kind === "publish" ? (
            <PublishGlyph />
          ) : (
            <Check />
          )}
          {action.kind === "sync" ? (
            <>
              Sync Changes
              <span className="flex items-center tabular-nums"><ArrowDown className="size-icon-xs" />{state?.behind}</span>
              <span className="flex items-center tabular-nums"><ArrowUp className="size-icon-xs" />{state?.ahead}</span>
            </>
          ) : action.kind === "publish" ? (
            "Publish Branch"
          ) : writing ? (
            "Writing message…"
          ) : (
            label
          )}
        </Button>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              size="sm"
              aria-label="Commit Actions"
              className="border-background/40 rounded-s-none border-s px-1.5"
              disabled={mainDisabled}
            >
              <ChevronDown />
            </Button>
          </DropdownMenuTrigger>
          <CommitActionsMenu blocked={blocked} onCommit={commit} />
        </DropdownMenu>
      </div>
      {!inRow && <BranchItem state={state} onClick={onBranch} disabled={locked} />}
      <StageAllDialog
        asking={asking !== null}
        onClose={() => setAsking(null)}
        onAnswer={(answer) => {
          const how = asking;
          setAsking(null);
          if (answer !== "yes") setStageAll(answer);
          if (how && answer !== "never") send(how, true);
        }}
      />
    </div>
  );
};

/** Commit with nothing staged: Cursor's question and answers. Escape leaves it unanswered. */
const StageAllDialog: FC<{ asking: boolean; onClose: () => void; onAnswer: (answer: "yes" | "always" | "never") => void }> = ({ asking, onClose, onAnswer }) => (
  <Dialog open={asking} onOpenChange={(open) => !open && onClose()}>
    <DialogContent data-slot="stage-all-dialog" showCloseButton={false} className="max-w-sm">
      <DialogTitle className="sr-only">Commit</DialogTitle>
      <DialogDescription className="text-foreground text-sm">
        There are no staged changes to commit. Would you like to stage all your changes and commit them directly?
      </DialogDescription>
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={() => onAnswer("never")}>Never</Button>
        <Button variant="secondary" size="sm" onClick={() => onAnswer("always")}>Always</Button>
        <Button size="sm" autoFocus onClick={() => onAnswer("yes")}>Yes</Button>
      </div>
    </DialogContent>
  </Dialog>
);

/**
 * The checked-out branch, "(merging)" during a merge, the branch being rebased with "(rebasing)"
 * during a rebase, and its commits not pushed yet; a click opens Checkout to….
 */
const BranchItem: FC<{ state: SourceState | null; onClick: (() => void) | null; disabled: boolean }> = ({
  state,
  onClick,
  disabled,
}) => {
  const content = (
    <>
      <Branch className="size-icon-sm shrink-0" />
      <span className="text-foreground min-w-0 truncate">
        {state?.branch ?? state?.rebasing?.branch ?? "…"}
        {state?.merging && " (merging)"}
        {state?.rebasing && " (rebasing)"}
      </span>
      {state && state.ahead > 0 && (
        <span title={`${state.ahead} not pushed`} className="flex shrink-0 items-center text-xs tabular-nums">
          <ArrowUp className="size-icon-xs" />
          {state.ahead}
        </span>
      )}
    </>
  );
  const look = "text-muted-foreground rounded-control flex h-control-sm min-w-0 items-center gap-1.5 px-1 text-sm";
  return onClick ? (
    <button
      type="button"
      title="Checkout to…"
      disabled={disabled}
      onClick={onClick}
      className={cn(look, "hover:bg-muted text-start disabled:opacity-50")}
    >
      {content}
    </button>
  ) : (
    <div data-slot="source-branch" className={look}>
      {content}
    </div>
  );
};
