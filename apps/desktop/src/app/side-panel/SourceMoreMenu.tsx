import type { CommitHow, Picking } from "@/app/side-panel/sourceTarget";
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger } from "@/components/overlays";
import type { SourceState } from "@/ipc/generated";

/**
 * The Source panel's More Actions menu: pull, push, fetch, commit, the changes, branches,
 * stashes and Git Output.
 */
export function SourceMoreMenu({
  state, merging, resolving, canCheckout, hasChanges, hasStaged, onPull, onPush, onFetch, onPick, onCommit, onAbortMerge,
  onAbortRebase, onStageAll, onUnstageAll, onDiscardAll, onCreateBranch, onRenameBranch, onPublish, onStash, onApplyStash,
  onDropStash, onShowGitOutput,
}: {
  state: SourceState | null;
  /** A merge is under way. */
  merging: boolean;
  /** A merge or rebase stopped on conflicts. */
  resolving: boolean;
  canCheckout: boolean;
  hasChanges: boolean;
  hasStaged: boolean;
  onPull: () => void;
  onPush: () => void;
  onFetch: () => void;
  onPick: (picking: Picking) => void;
  onCommit: (how: CommitHow) => void;
  onAbortMerge: () => void;
  onAbortRebase: () => void;
  onStageAll: () => void;
  onUnstageAll: () => void;
  onDiscardAll: () => void;
  onCreateBranch: () => void;
  onRenameBranch: () => void;
  onPublish: () => void;
  onStash: (untracked: boolean) => void;
  onApplyStash: (pop: boolean) => void;
  onDropStash: () => void;
  onShowGitOutput: () => void;
}) {
  return (
    <DropdownMenuContent align="end">
      <DropdownMenuItem onSelect={onPull}>Pull</DropdownMenuItem>
      <DropdownMenuItem onSelect={onPush}>Push</DropdownMenuItem>
      {canCheckout && <DropdownMenuItem onSelect={() => onPick("checkout")}>Checkout to…</DropdownMenuItem>}
      <DropdownMenuItem onSelect={onFetch}>Fetch</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>Commit</DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuItem onSelect={() => onCommit("commit")}>Commit</DropdownMenuItem>
          <DropdownMenuItem disabled={resolving} onSelect={() => onCommit("amend")}>
            Commit (Amend)
          </DropdownMenuItem>
          {merging && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={onAbortMerge}>Abort Merge</DropdownMenuItem>
            </>
          )}
          {state?.rebasing && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={onAbortRebase}>Abort Rebase</DropdownMenuItem>
            </>
          )}
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>Changes</DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuItem disabled={!hasChanges} onSelect={onStageAll}>
            Stage All Changes
          </DropdownMenuItem>
          <DropdownMenuItem disabled={!hasStaged} onSelect={onUnstageAll}>
            Unstage All Changes
          </DropdownMenuItem>
          <DropdownMenuItem disabled={!hasChanges} onSelect={onDiscardAll}>
            Discard All Changes
          </DropdownMenuItem>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>Pull, Push</DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuItem onSelect={onPull}>Pull</DropdownMenuItem>
          <DropdownMenuItem onSelect={onPush}>Push</DropdownMenuItem>
          <DropdownMenuItem onSelect={onFetch}>Fetch</DropdownMenuItem>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>Branch</DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuItem disabled={resolving} onSelect={() => onPick("merge")}>Merge…</DropdownMenuItem>
          <DropdownMenuItem disabled={resolving || !state?.branch} onSelect={() => onPick("rebase")}>
            Rebase Branch…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          {/* A session works on its own branch: it makes and renames none. */}
          {canCheckout && (
            <>
              <DropdownMenuItem onSelect={onCreateBranch}>Create Branch…</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => onPick("from")}>Create Branch From…</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem disabled={!state?.branch} onSelect={onRenameBranch}>
                Rename Branch…
              </DropdownMenuItem>
            </>
          )}
          <DropdownMenuItem onSelect={() => onPick("delete")}>Delete Branch…</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={!state?.branch || !!state.upstream} onSelect={onPublish}>
            Publish Branch
          </DropdownMenuItem>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>Stash</DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuItem disabled={resolving} onSelect={() => onStash(false)}>Stash</DropdownMenuItem>
          <DropdownMenuItem disabled={resolving} onSelect={() => onStash(true)}>
            Stash (Include Untracked)
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={resolving} onSelect={() => onApplyStash(false)}>
            Apply Latest Stash
          </DropdownMenuItem>
          <DropdownMenuItem disabled={resolving} onSelect={() => onApplyStash(true)}>
            Pop Latest Stash
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onDropStash}>Drop Stash…</DropdownMenuItem>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSeparator />
      <DropdownMenuItem onSelect={onShowGitOutput}>Show Git Output</DropdownMenuItem>
    </DropdownMenuContent>
  );
}
