import { type FC, type ReactNode, type RefObject, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { BranchNameBox, BranchPicker } from "@/app/conversation/BranchPicker";
import { conflictCount, parseConflicts } from "@/app/conversation/conflicts";
import { LETTERS } from "@/app/conversation/sourceStatus";
import {
  ArrowUp,
  Branch,
  Check,
  ChevronDown,
  ChevronRight,
  DotsHorizontal,
  FileDocument,
  FileIcon,
  Minus,
  Plus,
  Reload,
  Sparkles,
  Spinner,
  Undo,
} from "@/components/icons";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { request } from "@/ipc/client";
import { emitWorkspaceChanged, keepSource, onWorkspaceChanged, readSource, useCachedEntry, useCheckoutKey, workspaceKey, type WorkspaceTarget } from "@/state/workspaceCache";
import type { CommitOutcome, SourceAction, SourceBranch, SourceChange, SourceFile, SourceGitRun, SourceScope, SourceState } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useCheckoutChanges } from "@/state/board";
import { noteSourceState } from "@/state/review";
import { openConflictTab, openFileTab, openReviewTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * The Source panel, after the design reference's git panel: a commit message (blank: a fast
 * model writes one from the staged diff), Commit with its menu (Amend, & Push, & Sync), the
 * branch, and the changes as "Merge Changes", "Staged Changes" and "Changes", each file with
 * its letter (M, A, D, R, !…) to open as a diff in the Review tab (a conflicted one in its
 * conflict tab), stage or unstage, or discard. More Actions pulls, pushes, fetches, merges a
 * branch in or rebases onto one, makes, renames, deletes and publishes branches, stashes, and
 * shows Git Output (what it ran); a merge with conflicts is resolved file by file, then
 * committed with Continue, or aborted; a rebase's alike, commit by commit, Continue going on
 * to the next commit's.
 *
 * Before a session starts it shows its project's folder instead. A session that will work in
 * that folder can stage and commit there already (there is no Review tab yet); one that will
 * make its own copy only reads it.
 */

/** Whose changes the panel shows: a session's checkout, or the project folder of one not started. */
export type SourceTarget =
  | { type: "conversation"; conversationId: string }
  | { type: "project"; projectId: string; readOnly: boolean };

/** The panel's changes, sent for its target. */
function sourceCalls(target: SourceTarget) {
  return target.type === "conversation"
    ? sessionCalls(target.conversationId)
    : projectCalls(target.projectId);
}

function sessionCalls(conversationId: string) {
  return {
    change: (change: SourceChange): Promise<{ state: SourceState }> =>
      change.type === "stage"
        ? request({ method: "stageFiles", conversationId, paths: change.paths })
        : change.type === "unstage"
          ? request({ method: "unstageFiles", conversationId, paths: change.paths })
          : request({ method: "discardFiles", conversationId, scope: change.scope, paths: change.paths }),
    commit: (message: string | null, push: boolean, amend: boolean): Promise<{ outcome: CommitOutcome }> =>
      request({ method: "commitChanges", conversationId, message, includeUnstaged: false, push, amend }),
  };
}

function projectCalls(projectId: string) {
  return {
    change: (change: SourceChange): Promise<{ state: SourceState }> =>
      request({ method: "changeProjectSource", projectId, change }),
    commit: (message: string | null, push: boolean, amend: boolean): Promise<{ outcome: CommitOutcome }> =>
      request({ method: "commitProjectChanges", projectId, message, includeUnstaged: false, push, amend }),
  };
}

/** The session a target belongs to, if it has started. */
function sessionOf(target: SourceTarget): string | null {
  return target.type === "conversation" ? target.conversationId : null;
}

/** The checkout a target reads and changes. */
function workspaceTargetOf(target: SourceTarget): WorkspaceTarget {
  return target.type === "conversation"
    ? { type: "conversation", conversationId: target.conversationId }
    : { type: "project", projectId: target.projectId };
}

/** Each target's unsent commit message, kept while the app runs: closing or switching the pane keeps it. */
const unsent = new Map<string, string>();

/** The key a target's unsent message is kept under. */
function targetKey(target: SourceTarget): string {
  return target.type === "conversation" ? target.conversationId : `project:${target.projectId}`;
}

function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function dirName(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}


/**
 * The checkout's state: what was last read of it at once, read again now, when it changes, and as
 * the board changes it.
 */
function useSourceState(target: SourceTarget) {
  const conversationId = sessionOf(target);
  const projectId = target.type === "project" ? target.projectId : null;
  const cacheTarget = useMemo<WorkspaceTarget>(
    () => (conversationId ? { type: "conversation", conversationId } : { type: "project", projectId: projectId ?? "" }),
    [conversationId, projectId],
  );
  const key = useCheckoutKey(cacheTarget);
  const kept = useCachedEntry(key)?.source;
  const [read, setRead] = useState<{ key: string; state: SourceState } | null>(null);
  const state = kept ?? (read?.key === key ? read.state : null);
  const [failure, setFailure] = useState<{ key: string; error: string } | null>(null);
  const changeKey = useCheckoutChanges(conversationId ?? "");
  const load = useCallback(() => {
    readSource(cacheTarget)
      .then((next) => {
        if (workspaceKey(cacheTarget) !== key) return;
        setRead({ key, state: next });
        setFailure(null);
        if (conversationId) noteSourceState(conversationId, next);
      })
      .catch((error: unknown) => { if (workspaceKey(cacheTarget) === key) setFailure({ key, error: reason(error) }); });
  }, [cacheTarget, conversationId, key]);
  // A landing, Undo or Reapply changes the checkout at once.
  useEffect(() => {
    if (changeKey) load();
  }, [changeKey, load]);
  useEffect(() => {
    load();
    // Agents change files meanwhile: the sidebar's watch says when.
    const unsubscribe = onWorkspaceChanged(cacheTarget, load);
    window.addEventListener("focus", load);
    return () => {
      unsubscribe();
      window.removeEventListener("focus", load);
    };
  }, [load, cacheTarget]);
  const set = (next: SourceState) => {
    if (workspaceKey(cacheTarget) !== key) return;
    keepSource(cacheTarget, next, key);
    setRead({ key, state: next });
    setFailure(null);
    if (conversationId) noteSourceState(conversationId, next);
  };
  return { state, error: failure?.key === key ? failure.error : null, set, load, cacheTarget };
}

type Discarding = { scope: SourceScope; files: SourceFile[] };

/** A conflicted file (or all of them) the user asked to stage while it still has conflict markers. */
type StagingConflicts = { paths: string[]; marked: number };

const CONFLICTS_LEFT = "There are merge conflicts. Resolve them before committing your changes.";

/** What a branch pick is for, and what its picker asks. */
type Picking = "merge" | "checkout" | "rebase" | "from" | "delete";
const PICK_PLACEHOLDER: Record<Picking, string> = {
  merge: "Select a branch to merge from",
  checkout: "Select a branch to checkout",
  rebase: "Select a branch to rebase onto",
  from: "Select a branch to create the new branch from",
  delete: "Select a branch to delete",
};

/** A name box open: a new branch's (from what is checked out, or `from`), or the branch's new name. */
type Naming = { kind: "create"; from: string | null } | { kind: "rename" };

export function SourcePanel({ target }: { target: SourceTarget }) {
  const { state, error, set, load, cacheTarget } = useSourceState(target);
  const conversationId = sessionOf(target);
  const readOnly = target.type === "project" && target.readOnly;
  const calls = sourceCalls(target);
  const [busy, setBusy] = useState(false);
  // A commit reads the staged diff for its message, then commits the index: it stays as is.
  const [committing, setCommitting] = useState(false);
  const locked = busy || committing;
  const [discarding, setDiscarding] = useState<Discarding | null>(null);
  const [stagingConflicts, setStagingConflicts] = useState<StagingConflicts | null>(null);
  const [open, setOpen] = useState(true);
  const [picker, setPicker] = useState<Picking | null>(null);
  const [naming, setNaming] = useState<Naming | null>(null);
  const [droppingStash, setDroppingStash] = useState(false);
  // Git Output while it is open: null while it loads.
  const [gitOutput, setGitOutput] = useState<SourceGitRun[] | null | "closed">("closed");
  const [branches, setBranches] = useState<SourceBranch[] | null>(null);
  const [nowMs, setNowMs] = useState(0);
  // A diff opens in the session's Review tab; before it starts there is none.
  const review = conversationId
    ? (path: string, staged: boolean) => openReviewTab(conversationId, { type: "file", path, staged })
    : null;
  // Checkout is for the project folder: a session works on its own branch.
  const canCheckout = target.type === "project" && !readOnly;

  const act = (run: () => Promise<{ state: SourceState }>, failed: string) => {
    setBusy(true);
    return run()
      .then(({ state: next }) => set(next))
      .catch((cause: unknown) => toast(`${failed}: ${reason(cause)}`, { tone: "error" }))
      .finally(() => setBusy(false));
  };
  const stage = (paths: string[] | null) =>
    act(() => calls.change({ type: "stage", paths }), "Failed to stage");
  const unstage = (paths: string[] | null) =>
    act(() => calls.change({ type: "unstage", paths }), "Failed to unstage");
  const discard = ({ scope, files }: Discarding) =>
    void act(() => calls.change({ type: "discard", scope, paths: files.map((file) => file.path) }), "Failed to discard");

  const showConflicts = (paths: readonly string[]) => {
    const first = paths[0];
    toast(CONFLICTS_LEFT, {
      tone: "error",
      actions: conversationId && first ? [{ label: "Show Changes", run: () => openConflictTab(conversationId, first) }] : [],
    });
  };
  /**
   * Runs a git action from the panel; a merge or pull that stops on conflicts says so. Resolves
   * to the checkout after it, or null when it failed or stopped on conflicts.
   */
  const run = (action: SourceAction, failed: string, done?: (next: SourceState) => string | null) => {
    setBusy(true);
    return request({ method: "sourceAction", target: cacheTarget, action })
      .then(({ state: next }): SourceState | null => {
        set(next);
        emitWorkspaceChanged(cacheTarget);
        const conflicted = next.changes.filter((file) => file.status === "conflicted").map((file) => file.path);
        const merged = (action.type === "merge" || action.type === "pull" || action.type === "sync") && next.merging;
        const rebased = (action.type === "rebase" || action.type === "continueRebase") && next.rebasing;
        if ((merged || rebased || action.type === "applyStash") && conflicted.length > 0) {
          showConflicts(conflicted);
          return null;
        }
        const said = done?.(next);
        if (said) toast(said);
        return next;
      })
      .catch((cause: unknown) => {
        toast(`${failed}: ${reason(cause)}`, { tone: "error" });
        return null;
      })
      .finally(() => setBusy(false));
  };
  const pull = () => void run({ type: "pull" }, "Failed to pull", () => "Pulled");
  const push = () => void run({ type: "push" }, "Failed to push", (next) => `Pushed ${next.branch ?? "the branch"}`);
  const fetch = () => void run({ type: "fetch" }, "Failed to fetch", () => "Fetched");
  const abortMerge = () => void run({ type: "abortMerge" }, "Failed to abort the merge");
  const abortRebase = () => void run({ type: "abortRebase" }, "Failed to abort the rebase");
  // Commit & Sync, after its commit: pull, then push unless the pull stopped (no pull of the
  // commits GitHub kept after a squash merge: the push replaces them).
  const sync = () => void run({ type: "sync" }, "Failed to sync", (next) => `Pushed ${next.branch ?? "the branch"}`);

  const rebase = (onto: string) =>
    void run({ type: "rebase", branch: onto }, "Failed to rebase", (next) => `Rebased ${next.branch ?? "the branch"} onto ${onto}`);
  // Continue during a rebase: the commit it stopped on, then on to the next commit's conflicts or its end.
  const continueRebase = () => {
    const onto = state?.rebasing?.onto ?? "";
    void run({ type: "continueRebase" }, "Failed to continue the rebase", (next) =>
      next.rebasing ? null : `Rebased ${next.branch ?? "the branch"} onto ${onto}`,
    );
  };
  const publish = () => void run({ type: "push" }, "Failed to publish the branch", (next) => `Published ${next.branch ?? "the branch"}`);
  const stash = (untracked: boolean) => void run({ type: "stash", untracked }, "Failed to stash", () => "Stashed the changes");
  const applyStash = (pop: boolean) => void run({ type: "applyStash", pop }, pop ? "Failed to pop the stash" : "Failed to apply the stash");
  const dropStash = () => void run({ type: "dropStash" }, "Failed to drop the stash", () => "Dropped the latest stash");
  /** Deletes a branch; one git won't delete as not merged is offered again, to delete anyway. */
  const deleteBranch = (name: string, force: boolean) => {
    setBusy(true);
    request({ method: "sourceAction", target: cacheTarget, action: { type: "deleteBranch", name, force } })
      .then(({ state: next }) => {
        set(next);
        emitWorkspaceChanged(cacheTarget);
        toast(`Deleted ${name}`);
      })
      .catch((cause: unknown) => {
        const said = reason(cause);
        if (!force && /not fully merged/.test(said)) {
          toast(`${name} isn't merged yet. Delete it anyway? Its commits are lost.`, {
            tone: "error",
            actions: [{ label: "Delete Anyway", run: () => deleteBranch(name, true) }],
          });
        } else toast(`Failed to delete the branch: ${said}`, { tone: "error" });
      })
      .finally(() => setBusy(false));
  };
  const named = (name: string) => {
    const which = naming;
    setNaming(null);
    if (locked || !which) return;
    if (which.kind === "create") void run({ type: "createBranch", name, from: which.from }, "Failed to create the branch");
    else void run({ type: "renameBranch", name }, "Failed to rename the branch");
  };
  const showGitOutput = () => {
    setGitOutput(null);
    request({ method: "getSourceGitOutput", target: cacheTarget })
      // Closed while it loaded: it stays closed.
      .then(({ runs }) => setGitOutput((now) => (now === null ? runs : now)))
      .catch((cause: unknown) => {
        setGitOutput("closed");
        toast(`Failed to read Git Output: ${reason(cause)}`, { tone: "error" });
      });
  };

  const pick = (which: Picking) => {
    setPicker(which);
    setBranches(null);
    setNowMs(Date.now());
    request({ method: "listSourceBranches", target: cacheTarget })
      .then(({ branches: listed }) => setBranches(listed))
      .catch((cause: unknown) => {
        setPicker(null);
        toast(`Failed to list branches: ${reason(cause)}`, { tone: "error" });
      });
  };
  const picked = (branch: SourceBranch) => {
    const which = picker;
    setPicker(null);
    // A commit under way commits whichever branch is checked out by then.
    if (locked) return;
    if (which === "merge") void run({ type: "merge", branch: branch.name }, "Failed to merge", () => `Merged ${branch.name}`);
    else if (which === "checkout") void run({ type: "checkout", branch: branch.name }, "Failed to check out");
    else if (which === "rebase") rebase(branch.name);
    else if (which === "from") setNaming({ kind: "create", from: branch.name });
    else if (which === "delete") deleteBranch(branch.name, false);
  };

  /**
   * Stages conflicted files, asking first when any still has conflict markers. A file that
   * can't be read as text (a binary one) has none to check.
   */
  const stageConflicted = (paths: string[]) => {
    setBusy(true);
    Promise.allSettled(paths.map((path) => request({ method: "getConflictFile", target: cacheTarget, path })))
      .then((read) => {
        const marked = read.filter(
          (entry) => entry.status === "fulfilled" && entry.value.file.text !== null && conflictCount(parseConflicts(entry.value.file.text)) > 0,
        ).length;
        if (marked > 0) {
          setStagingConflicts({ paths, marked });
          return;
        }
        return stage(paths);
      })
      .finally(() => setBusy(false));
  };

  const staged = state?.staged ?? [];
  const all = state?.changes ?? [];
  const conflicted = all.filter((file) => file.status === "conflicted");
  const changes = all.filter((file) => file.status !== "conflicted");
  const unstagedPaths = new Set(all.map((file) => file.path));
  const merging = state?.merging ?? null;
  // A merge or rebase stopped on conflicts: the branch stays as it is till it ends.
  const resolving = merging !== null || state?.rebasing != null;
  const openGroup = (scope: "staged" | "unstaged") => {
    if (conversationId) openReviewTab(conversationId, { type: "changes", staged: scope === "staged" });
  };
  const openFile = conversationId ? (path: string) => openFileTab(conversationId, path) : null;
  const commitRef = useRef<((how: CommitHow) => void) | null>(null);

  return (
    <div data-slot="source-panel" className="relative flex min-h-0 flex-1 flex-col">
      <header className="group/source-title text-muted-foreground flex h-control-md shrink-0 items-center gap-0.5 ps-1 pe-2">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="flex min-w-0 flex-1 items-center gap-1 text-start text-xs font-semibold tracking-wide"
        >
          {open ? <ChevronDown className="size-icon-sm shrink-0" /> : <ChevronRight className="size-icon-sm shrink-0" />}
          SOURCE
        </button>
        <span className="flex items-center opacity-0 transition-opacity group-hover/source-title:opacity-100 group-focus-within/source-title:opacity-100">
          {!readOnly && (
            <TooltipIconButton tooltip="Commit" size="icon-xs" disabled={locked} onClick={() => commitRef.current?.("commit")}>
              <Check />
            </TooltipIconButton>
          )}
          <TooltipIconButton tooltip="Refresh" size="icon-xs" onClick={load}>
            <Reload />
          </TooltipIconButton>
        </span>
        {!readOnly && (
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <TooltipIconButton tooltip="More Actions" size="icon-xs" disabled={locked}>
                <DotsHorizontal />
              </TooltipIconButton>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={pull}>Pull</DropdownMenuItem>
              <DropdownMenuItem onSelect={push}>Push</DropdownMenuItem>
              {canCheckout && <DropdownMenuItem onSelect={() => pick("checkout")}>Checkout to…</DropdownMenuItem>}
              <DropdownMenuItem onSelect={fetch}>Fetch</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Commit</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  <DropdownMenuItem onSelect={() => commitRef.current?.("commit")}>Commit</DropdownMenuItem>
                  <DropdownMenuItem disabled={resolving} onSelect={() => commitRef.current?.("amend")}>
                    Commit (Amend)
                  </DropdownMenuItem>
                  {merging && (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem onSelect={abortMerge}>Abort Merge</DropdownMenuItem>
                    </>
                  )}
                  {state?.rebasing && (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem onSelect={abortRebase}>Abort Rebase</DropdownMenuItem>
                    </>
                  )}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Changes</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  <DropdownMenuItem disabled={changes.length === 0} onSelect={() => stage(changes.map((file) => file.path))}>
                    Stage All Changes
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={staged.length === 0} onSelect={() => unstage(null)}>
                    Unstage All Changes
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={changes.length === 0} onSelect={() => setDiscarding({ scope: "unstaged", files: changes })}>
                    Discard All Changes
                  </DropdownMenuItem>
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Pull, Push</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  <DropdownMenuItem onSelect={pull}>Pull</DropdownMenuItem>
                  <DropdownMenuItem onSelect={push}>Push</DropdownMenuItem>
                  <DropdownMenuItem onSelect={fetch}>Fetch</DropdownMenuItem>
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Branch</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  <DropdownMenuItem disabled={resolving} onSelect={() => pick("merge")}>Merge…</DropdownMenuItem>
                  <DropdownMenuItem disabled={resolving || !state?.branch} onSelect={() => pick("rebase")}>
                    Rebase Branch…
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  {/* A session works on its own branch: it makes and renames none. */}
                  {canCheckout && (
                    <>
                      <DropdownMenuItem onSelect={() => setNaming({ kind: "create", from: null })}>Create Branch…</DropdownMenuItem>
                      <DropdownMenuItem onSelect={() => pick("from")}>Create Branch From…</DropdownMenuItem>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem disabled={!state?.branch} onSelect={() => setNaming({ kind: "rename" })}>
                        Rename Branch…
                      </DropdownMenuItem>
                    </>
                  )}
                  <DropdownMenuItem onSelect={() => pick("delete")}>Delete Branch…</DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem disabled={!state?.branch || !!state.upstream} onSelect={publish}>
                    Publish Branch
                  </DropdownMenuItem>
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Stash</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  <DropdownMenuItem disabled={resolving} onSelect={() => stash(false)}>Stash</DropdownMenuItem>
                  <DropdownMenuItem disabled={resolving} onSelect={() => stash(true)}>
                    Stash (Include Untracked)
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem disabled={resolving} onSelect={() => applyStash(false)}>
                    Apply Latest Stash
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={resolving} onSelect={() => applyStash(true)}>
                    Pop Latest Stash
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onSelect={() => setDroppingStash(true)}>Drop Stash…</DropdownMenuItem>
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={showGitOutput}>Show Git Output</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </header>
      {picker && (
        <BranchPicker
          placeholder={PICK_PLACEHOLDER[picker]}
          branches={
            branches &&
            branches.filter((branch) =>
              picker === "from" ? true : picker === "delete" ? !branch.remote && branch.name !== state?.branch : branch.remote || branch.name !== state?.branch,
            )
          }
          nowMs={nowMs}
          onPick={picked}
          onClose={() => setPicker(null)}
        />
      )}
      {naming && (
        <BranchNameBox
          placeholder={naming.kind === "create" ? "New branch name" : "New name for the branch"}
          hint={
            naming.kind === "rename"
              ? `Enter renames ${state?.branch ?? "the branch"}`
              : `Enter creates it from ${naming.from ?? state?.branch ?? "what is checked out"} and switches to it`
          }
          initial={naming.kind === "rename" ? (state?.branch ?? "") : ""}
          onName={named}
          onClose={() => setNaming(null)}
        />
      )}
      {/* Folded, it stays mounted: a commit under way keeps its state. */}
      <div className={cn("flex min-h-0 flex-1 flex-col", !open && "hidden")}>
          {readOnly ? (
            <OwnCopyNote state={state} />
          ) : (
            <CommitBox
              target={target}
              state={state}
              staging={busy}
              commitRef={commitRef}
              onCommitting={setCommitting}
              onCommitted={load}
              onSync={sync}
              onContinueRebase={continueRebase}
              onBranch={canCheckout ? () => pick("checkout") : null}
              locked={locked}
            />
          )}
          <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
            {error && !state && <p className="text-destructive p-2 text-sm">{error}</p>}
            {state && staged.length === 0 && all.length === 0 && !resolving && (
              <p className="text-muted-foreground p-2 text-sm">No changes</p>
            )}
            {conflicted.length > 0 && (
              <Group
                title="Merge Changes"
                count={conflicted.length}
                actions={
                  !readOnly && (
                    <TooltipIconButton
                      tooltip="Stage All Merge Changes"
                      size="icon-xs"
                      disabled={locked}
                      onClick={() => stageConflicted(conflicted.map((file) => file.path))}
                    >
                      <Plus />
                    </TooltipIconButton>
                  )
                }
              >
                {conflicted.map((file) => (
                  <FileRow
                    key={`merge:${file.path}`}
                    file={file}
                    onOpen={conversationId ? () => openConflictTab(conversationId, file.path) : null}
                  >
                    {!readOnly && (
                      <>
                        {openFile && <OpenFileButton onClick={() => openFile(file.path)} />}
                        <TooltipIconButton
                          tooltip="Stage Changes"
                          size="icon-xs"
                          disabled={locked}
                          onClick={() => stageConflicted([file.path])}
                        >
                          <Plus />
                        </TooltipIconButton>
                      </>
                    )}
                  </FileRow>
                ))}
              </Group>
            )}
            {staged.length > 0 && (
              <Group
                title="Staged Changes"
                count={staged.length}
                actions={
                  <>
                    {conversationId && (
                      <TooltipIconButton tooltip="Open Staged Changes" size="icon-xs" onClick={() => openGroup("staged")}>
                        <FileDocument />
                      </TooltipIconButton>
                    )}
                    {!readOnly && (
                      <TooltipIconButton tooltip="Unstage All Changes" size="icon-xs" disabled={locked} onClick={() => unstage(null)}>
                        <Minus />
                      </TooltipIconButton>
                    )}
                  </>
                }
              >
                {staged.map((file) => (
                  <FileRow key={`staged:${file.path}`} file={file} onOpen={review ? () => review(file.path, true) : null}>
                    {openFile && file.status !== "deleted" && <OpenFileButton onClick={() => openFile(file.path)} />}
                    {!readOnly && (
                      <TooltipIconButton
                        tooltip="Unstage Changes"
                        size="icon-xs"
                        disabled={locked}
                        onClick={() => unstage([file.path])}
                      >
                        <Minus />
                      </TooltipIconButton>
                    )}
                  </FileRow>
                ))}
              </Group>
            )}
            {state && (changes.length > 0 || staged.length > 0 || resolving) && (
              <Group
                title="Changes"
                count={changes.length}
                actions={
                  changes.length > 0 && (
                    <>
                      {conversationId && (
                        <TooltipIconButton tooltip="Open Changes" size="icon-xs" onClick={() => openGroup("unstaged")}>
                          <FileDocument />
                        </TooltipIconButton>
                      )}
                      {!readOnly && (
                        <>
                          <TooltipIconButton
                            tooltip="Discard All Changes"
                            size="icon-xs"
                            disabled={locked}
                            onClick={() => setDiscarding({ scope: "unstaged", files: changes })}
                          >
                            <Undo />
                          </TooltipIconButton>
                          <TooltipIconButton
                            tooltip="Stage All Changes"
                            size="icon-xs"
                            disabled={locked}
                            onClick={() => stage(changes.map((file) => file.path))}
                          >
                            <Plus />
                          </TooltipIconButton>
                        </>
                      )}
                    </>
                  )
                }
              >
                {changes.map((file) => (
                  <FileRow key={`changes:${file.path}`} file={file} onOpen={review ? () => review(file.path, false) : null}>
                    {openFile && file.status !== "deleted" && <OpenFileButton onClick={() => openFile(file.path)} />}
                    {!readOnly && (
                      <>
                        <TooltipIconButton
                          tooltip="Discard Changes"
                          size="icon-xs"
                          disabled={locked}
                          onClick={() => setDiscarding({ scope: "unstaged", files: [file] })}
                        >
                          <Undo />
                        </TooltipIconButton>
                        <TooltipIconButton
                          tooltip="Stage Changes"
                          size="icon-xs"
                          disabled={locked}
                          onClick={() => stage([file.path])}
                        >
                          <Plus />
                        </TooltipIconButton>
                      </>
                    )}
                  </FileRow>
                ))}
              </Group>
            )}
          </div>
      </div>
      <DiscardDialog
        discarding={discarding}
        unstagedPaths={unstagedPaths}
        onCancel={() => setDiscarding(null)}
        onConfirm={() => {
          if (discarding) discard(discarding);
          setDiscarding(null);
        }}
      />
      <Dialog open={droppingStash} onOpenChange={setDroppingStash}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Drop the latest stash?</DialogTitle>
            <DialogDescription>Its changes are deleted for good.</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setDroppingStash(false)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                setDroppingStash(false);
                dropStash();
              }}
            >
              Drop
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <GitOutputDialog runs={gitOutput} onClose={() => setGitOutput("closed")} />
      <Dialog open={stagingConflicts !== null} onOpenChange={(next) => !next && setStagingConflicts(null)}>
        <DialogContent>
          {stagingConflicts && (
            <>
              <DialogHeader>
                <DialogTitle>
                  {stagingConflicts.paths.length === 1
                    ? `Stage ${baseName(stagingConflicts.paths[0]!)} with its conflicts?`
                    : `Stage ${stagingConflicts.marked} ${stagingConflicts.marked === 1 ? "file" : "files"} with conflicts?`}
                </DialogTitle>
                <DialogDescription>
                  {stagingConflicts.paths.length === 1 ? "It still has" : "They still have"} conflict markers. Staging marks
                  the conflicts resolved, and the markers would be committed as they are.
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <Button variant="ghost" onClick={() => setStagingConflicts(null)}>
                  Cancel
                </Button>
                <Button
                  onClick={() => {
                    void stage(stagingConflicts.paths);
                    setStagingConflicts(null);
                  }}
                >
                  Stage
                </Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** A Git Output entry as text: its time, the command, then what it printed. */
function gitRunText(run: SourceGitRun): string {
  return `${formatTime(run.atMs)}  > ${run.command}${run.output ? `\n${run.output}` : ""}`;
}

/**
 * Git Output: the git commands the panel ran on this checkout, oldest first, a failure's output
 * in red; Copy takes it all, for a bug report.
 */
const GitOutputDialog: FC<{ runs: SourceGitRun[] | null | "closed"; onClose: () => void }> = ({ runs, onClose }) => (
  <Dialog open={runs !== "closed"} onOpenChange={(open) => !open && onClose()}>
    <DialogContent data-slot="git-output" className="max-w-git-output">
      <DialogHeader>
        <DialogTitle>Git Output</DialogTitle>
        <DialogDescription>What the Source panel ran here, newest last.</DialogDescription>
      </DialogHeader>
      <pre className="bg-muted/40 rounded-control text-foreground/80 max-h-96 overflow-auto p-3 font-mono text-xs leading-5 whitespace-pre-wrap">
        {runs === null && <span className="text-muted-foreground">Loading…</span>}
        {runs !== null && runs !== "closed" && runs.length === 0 && <span className="text-muted-foreground">Nothing ran yet.</span>}
        {runs !== null &&
          runs !== "closed" &&
          runs.map((run, index) => (
            <span key={`${run.atMs}-${index}`} data-slot="git-output-run" className="block pb-2">
              <span className="text-muted-foreground">{formatTime(run.atMs)} </span>
              <span className="text-foreground">&gt; {run.command}</span>
              {run.output && "\n"}
              {run.output && <span className={run.ok ? undefined : "text-destructive"}>{run.output}</span>}
            </span>
          ))}
      </pre>
      <div className="flex justify-end gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={runs === null || runs === "closed" || runs.length === 0}
          onClick={() => {
            if (runs === null || runs === "closed") return;
            void navigator.clipboard.writeText(runs.map(gitRunText).join("\n\n")).then(() => toast("Copied Git Output"));
          }}
        >
          Copy
        </Button>
      </div>
    </DialogContent>
  </Dialog>
);

const OpenFileButton: FC<{ onClick: () => void }> = ({ onClick }) => (
  <TooltipIconButton tooltip="Open File" size="icon-xs" onClick={onClick}>
    <FileDocument />
  </TooltipIconButton>
);

/** A group of files under its title and count, folding away; its actions show on hover. */
const Group: FC<{ title: string; count: number; actions: ReactNode; children: ReactNode }> = ({
  title,
  count,
  actions,
  children,
}) => {
  const [open, setOpen] = useState(true);
  return (
    <section aria-label={title} className="mt-2">
      <header className="group/source-group flex h-control-sm items-center gap-1 pe-1 text-sm">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="flex min-w-0 flex-1 items-center gap-1 text-start"
        >
          {open ? (
            <ChevronDown className="text-muted-foreground size-icon-sm shrink-0" />
          ) : (
            <ChevronRight className="text-muted-foreground size-icon-sm shrink-0" />
          )}
          <span className="truncate">{title}</span>
        </button>
        <span className="hidden shrink-0 items-center group-focus-within/source-group:flex group-hover/source-group:flex">
          {actions}
        </span>
        <span className="bg-muted text-muted-foreground rounded-full px-1.5 text-xs tabular-nums">{count}</span>
      </header>
      {open && <ul>{children}</ul>}
    </section>
  );
};

/** A changed file: click to open its diff (once there is a Review tab); its actions show on hover. */
const FileRow: FC<{ file: SourceFile; onOpen: (() => void) | null; children: ReactNode }> = ({
  file,
  onOpen,
  children,
}) => {
  const status = LETTERS[file.status];
  const name = (
    <>
      <FileIcon name={file.path} />
      <span
        className={cn(
          "shrink-0",
          file.status === "deleted" && "line-through",
          file.status === "conflicted" && "text-destructive",
        )}
      >
        {baseName(file.path)}
      </span>
      <span className="text-muted-foreground min-w-0 truncate text-xs">{dirName(file.path)}</span>
    </>
  );
  const title = file.oldPath ? `${file.oldPath} → ${file.path}` : file.path;
  const look = "flex min-w-0 flex-1 items-center gap-1.5 rounded-sm ps-6 text-start text-sm";
  return (
    <li className="group/source-row hover:bg-muted rounded-control flex h-control-sm items-center gap-1 pe-1">
      {onOpen ? (
        <button type="button" title={title} onClick={onOpen} className={look}>
          {name}
        </button>
      ) : (
        <span title={title} className={look}>
          {name}
        </span>
      )}
      <span className="hidden shrink-0 items-center group-focus-within/source-row:flex group-hover/source-row:flex">
        {children}
      </span>
      <span
        title={status.label}
        className={cn("w-4 shrink-0 text-center font-mono text-xs font-medium", status.className)}
      >
        {status.letter}
      </span>
    </li>
  );
};

/** What discarding `file` throws away, in plain words. */
function discardWords(scope: SourceScope, file: SourceFile, alsoUnstaged: boolean): string {
  if (scope === "unstaged") {
    return file.status === "untracked"
      ? `${file.path} is deleted. Git doesn't track it, so it can't be brought back.`
      : `The changes to ${file.path} that aren't staged are lost. Its staged changes stay.`;
  }
  const back =
    file.status === "added"
      ? `${file.path} is new, so it's deleted.`
      : file.status === "renamed"
        ? `${file.path} goes back to ${file.oldPath ?? "its old name"} as it was last committed.`
        : `${file.path} goes back to how it was last committed.`;
  return alsoUnstaged ? `${back} Its changes that aren't staged are lost too.` : back;
}

const DiscardDialog: FC<{
  discarding: Discarding | null;
  unstagedPaths: ReadonlySet<string>;
  onCancel: () => void;
  onConfirm: () => void;
}> = ({ discarding, unstagedPaths, onCancel, onConfirm }) => {
  const file = discarding?.files[0];
  const many = (discarding?.files.length ?? 0) > 1;
  return (
    <Dialog open={discarding !== null} onOpenChange={(open) => !open && onCancel()}>
      <DialogContent>
        {discarding && file && (
          <>
            <DialogHeader>
              <DialogTitle>
                {many
                  ? `Discard all changes in ${discarding.files.length} files?`
                  : discarding.scope === "unstaged" && file.status === "untracked"
                    ? `Delete ${baseName(file.path)}?`
                    : `Discard ${discarding.scope === "staged" ? "staged " : ""}changes to ${baseName(file.path)}?`}
              </DialogTitle>
              <DialogDescription>
                {many
                  ? "Their changes that aren't staged are lost, and new files Git doesn't track are deleted. This can't be undone."
                  : discardWords(
                      discarding.scope,
                      file,
                      discarding.scope === "staged" && unstagedPaths.has(file.path),
                    )}
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="ghost" onClick={onCancel}>
                Cancel
              </Button>
              <Button variant="destructive" onClick={onConfirm}>
                {!many && discarding.scope === "unstaged" && file.status === "untracked" ? "Delete" : "Discard"}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
};

/** How the Commit button's menu commits: plain, amending the last commit, then pushing, or then pulling and pushing. */
type CommitHow = "commit" | "amend" | "push" | "sync";

/**
 * The message box (blank: written for you; ✨ writes one now; during a merge, git's merge
 * message; during a rebase, the message of the commit it stopped on, kept as it is), the Commit
 * button with its menu (Continue during a merge or rebase), and the branch.
 */
const CommitBox: FC<{
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
}> = ({ target, state, staging, commitRef, onCommitting, onCommitted, onSync, onContinueRebase, onBranch, locked }) => {
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
  return (
    <div className="flex shrink-0 flex-col gap-2 px-2 pb-1">
      <div className="border-border rounded-control focus-within:border-ring flex items-start border">
        <textarea
          rows={1}
          value={rebasing ? rebasing.message : message}
          disabled={busy !== null || rebasing !== null}
          aria-label="Commit message"
          placeholder={`Message (${mac ? "⌘⏎" : "Ctrl+⏎"} to commit on "${state?.branch ?? "…"}")`}
          onChange={(event) => setMessage(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (mac ? event.metaKey : event.ctrlKey)) {
              event.preventDefault();
              commit("commit");
            }
          }}
          className="placeholder:text-muted-foreground field-sizing-content max-h-40 min-w-0 flex-1 resize-none bg-transparent px-2 py-1.5 text-sm outline-none"
        />
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
          <DropdownMenuContent align="end">
            <DropdownMenuItem disabled={blocked("commit")} onSelect={() => commit("commit")}>
              Commit
            </DropdownMenuItem>
            <DropdownMenuItem disabled={blocked("amend")} onSelect={() => commit("amend")}>
              Commit (Amend)
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem disabled={blocked("push")} onSelect={() => commit("push")}>
              Commit & Push
            </DropdownMenuItem>
            <DropdownMenuItem disabled={blocked("sync")} onSelect={() => commit("sync")}>
              Commit & Sync
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
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

/**
 * Over a project folder's changes when the session will make its own copy: the folder's branch,
 * and that the session's work won't land here, so nothing here can be changed from this pane.
 */
const OwnCopyNote: FC<{ state: SourceState | null }> = ({ state }) => (
  <div className="border-border flex shrink-0 flex-col gap-1 border-b p-2">
    <div className="text-muted-foreground flex h-control-sm items-center gap-1.5 px-1 text-sm">
      <Branch className="size-icon-sm shrink-0" />
      <span className="text-foreground min-w-0 truncate">{state?.branch ?? "…"}</span>
    </div>
    <p data-slot="own-copy-note" className="text-muted-foreground px-1 text-xs">
      This session will work in its own copy
    </p>
  </div>
);
