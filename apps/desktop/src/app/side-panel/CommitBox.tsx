import { type FC, type RefObject, useEffect, useState } from "react";

import { CommitActionsMenu } from "@/app/side-panel/CommitActionsMenu";
import { type CommitHow, CONFLICTS_LEFT, reason, sourceCalls, type SourceTarget, targetKey, unsent, workspaceTargetOf } from "@/app/side-panel/sourceTarget";
import { ArrowUp, Branch, Check, ChevronDown, Sparkles, Spinner } from "@/components/icons";
import { TooltipIconButton, Button } from "@/components/controls";
import { DropdownMenu, DropdownMenuTrigger } from "@/components/overlays";
import { request } from "@/ipc/client";
import type { SourceState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * The message box (blank: written for you; ✨ writes one now; during a merge, git's merge
 * message; during a rebase, the message of the commit it stopped on, kept as it is), the Commit
 * button with its menu (Continue during a merge or rebase), and the branch. With nothing to
 * commit, only the branch.
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
  /** Commit & Sync's pull and push, after the commit. */
  onSync: () => void;
  /** Continue during a rebase. */
  onContinueRebase: () => void;
  /** Opens Checkout to…; null where the branch can't change. */
  onBranch: (() => void) | null;
  /** Something is under way: the branch can't change meanwhile. */
  locked: boolean;
  /** Nothing to commit: only the branch shows (unless a commit is under way). */
  idle?: boolean;
}> = ({ target, state, staging, commitRef, onCommitting, onCommitted, onSync, onContinueRebase, onBranch, locked, idle = false }) => {
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
  const nothing = !state || (merging || rebasing ? conflicted : state.staged.length === 0);
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
    setBusy(how);
    sourceCalls(target)
      .commit(message.trim() || null, how === "push", how === "amend")
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
    request({ method: "writeCommitMessage", target: workspaceTargetOf(target) })
      .then(({ message: written }) => setMessage(written))
      .catch((failure: unknown) => toast(`Failed to write a message: ${reason(failure)}`, { tone: "error" }))
      .finally(() => setBusy(null));
  };
  const writing = busy !== null && busy !== "write" && !message.trim() && !merging;
  const label = merging || rebasing ? "Continue" : "Commit";
  const placeholder = `Message (${mac ? "⌘⏎" : "Ctrl+⏎"} to commit on "${state?.branch ?? "…"}")`;
  return (
    <div className="flex shrink-0 flex-col gap-2 px-2 pb-1">
      {(!idle || busy !== null) && (
        <>
          <div className="border-border rounded-control focus-within:border-ring flex items-start border">
            {/* An empty box is as tall as its placeholder: WebKit sizes it to one line (#490). */}
            <div className="grid min-w-0 flex-1">
              <span aria-hidden className="invisible col-start-1 row-start-1 max-h-40 overflow-hidden px-2 py-1.5 text-sm break-words whitespace-pre-wrap">
                {(rebasing ? rebasing.message : message) ? "" : placeholder}
              </span>
              <textarea
                rows={1}
                value={rebasing ? rebasing.message : message}
                disabled={busy !== null || rebasing !== null}
                aria-label="Commit message"
                placeholder={placeholder}
                onChange={(event) => setMessage(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && (mac ? event.metaKey : event.ctrlKey)) {
                    event.preventDefault();
                    commit("commit");
                  }
                }}
                className="placeholder:text-muted-foreground field-sizing-content col-start-1 row-start-1 max-h-40 w-full min-w-0 resize-none bg-transparent px-2 py-1.5 text-sm outline-none"
              />
            </div>
            <TooltipIconButton
              tooltip="Generate Commit Message"
              size="icon-xs"
              className="m-1"
              disabled={busy !== null || merging !== null || rebasing !== null || !state || state.staged.length === 0}
              onClick={write}
            >
              {busy === "write" ? <Spinner className="size-icon-xs animate-spin motion-reduce:animate-none" /> : <Sparkles />}
            </TooltipIconButton>
          </div>
          <div className="flex">
            <Button
              size="sm"
              className="flex-1 rounded-e-none"
              disabled={nothing || busy !== null || staging}
              title={(merging || rebasing) && conflicted ? "Resolve the merge conflicts first" : nothing ? "Stage changes to commit them" : undefined}
              onClick={() => commit("commit")}
            >
              {busy === "commit" ? <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" /> : <Check />}
              {writing ? "Writing message…" : label}
            </Button>
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild>
                <Button
                  size="sm"
                  aria-label="Commit Actions"
                  className="border-background/40 rounded-s-none border-s px-1.5"
                  disabled={busy !== null || staging}
                >
                  <ChevronDown />
                </Button>
              </DropdownMenuTrigger>
              <CommitActionsMenu blocked={blocked} onCommit={commit} />
            </DropdownMenu>
          </div>
        </>
      )}
      <BranchItem state={state} onClick={onBranch} disabled={locked} />
    </div>
  );
};

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
