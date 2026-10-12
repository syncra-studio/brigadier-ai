import {
  ArrowUp,
  Branch,
  Check,
  Commit,
  Minus,
  Plus,
  Undo,
  UploadDocuments,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { LETTERS } from "@/app/conversation/sourceStatus";
import { FileIcon } from "@/components/glyphs/fileIcons";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import { request } from "@/ipc/client";
import { keepSource, onWorkspaceChanged, readSource, useCachedEntry, useCheckoutKey, workspaceKey, type WorkspaceTarget } from "@/state/workspaceCache";
import type { CommitOutcome, SourceChange, SourceFile, SourceScope, SourceState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useCheckoutChanges } from "@/state/board";
import { noteSourceState, setReviewScope } from "@/state/review";
import { openReviewTab } from "@/state/sessionTabs";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * The Source panel: the session checkout's changes as "Staged" and "Changes", each file with
 * its letter (M, A, D, R…), to open as a diff in the Review tab, stage or unstage, or discard;
 * then a commit message (blank: a fast model writes one from the staged diff), Commit, and
 * Commit & push when the branch has a remote.
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
    commit: (message: string | null, push: boolean): Promise<{ outcome: CommitOutcome }> =>
      request({ method: "commitChanges", conversationId, message, includeUnstaged: false, push }),
  };
}

function projectCalls(projectId: string) {
  return {
    change: (change: SourceChange): Promise<{ state: SourceState }> =>
      request({ method: "changeProjectSource", projectId, change }),
    commit: (message: string | null, push: boolean): Promise<{ outcome: CommitOutcome }> =>
      request({ method: "commitProjectChanges", projectId, message, includeUnstaged: false, push }),
  };
}

/** The session a target belongs to, if it has started. */
function sessionOf(target: SourceTarget): string | null {
  return target.type === "conversation" ? target.conversationId : null;
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
  return { state, error: failure?.key === key ? failure.error : null, set, load };
}

type Discarding = { scope: SourceScope; files: SourceFile[] };

export function SourcePanel({ target }: { target: SourceTarget }) {
  const { state, error, set, load } = useSourceState(target);
  const conversationId = sessionOf(target);
  const readOnly = target.type === "project" && target.readOnly;
  const calls = sourceCalls(target);
  const [busy, setBusy] = useState(false);
  // A commit reads the staged diff for its message, then commits the index: it stays as is.
  const [committing, setCommitting] = useState(false);
  const locked = busy || committing;
  const [discarding, setDiscarding] = useState<Discarding | null>(null);
  // A diff opens in the session's Review tab; before it starts there is none.
  const review = conversationId
    ? (path: string, staged: boolean) => openReviewTab(conversationId, { type: "file", path, staged })
    : null;

  const act = (run: () => Promise<{ state: SourceState }>, failed: string) => {
    setBusy(true);
    run()
      .then(({ state: next }) => set(next))
      .catch((cause: unknown) => toast(`${failed}: ${reason(cause)}`, { tone: "error" }))
      .finally(() => setBusy(false));
  };
  const stage = (paths: string[] | null) =>
    act(() => calls.change({ type: "stage", paths }), "Failed to stage");
  const unstage = (paths: string[] | null) =>
    act(() => calls.change({ type: "unstage", paths }), "Failed to unstage");
  const discard = ({ scope, files }: Discarding, all: boolean) =>
    act(
      () => calls.change({ type: "discard", scope, paths: all ? null : files.map((file) => file.path) }),
      "Failed to discard",
    );

  const staged = state?.staged ?? [];
  const changes = state?.changes ?? [];
  const unstagedPaths = new Set(changes.map((file) => file.path));

  return (
    <div data-slot="source-panel" className="flex min-h-0 flex-1 flex-col">
      {readOnly ? (
        <OwnCopyNote state={state} />
      ) : (
        <CommitBox
          target={target}
          state={state}
          staging={busy}
          onCommitting={setCommitting}
          onCommitted={load}
        />
      )}
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {error && !state && <p className="text-destructive p-2 text-sm">{error}</p>}
        {state && staged.length === 0 && changes.length === 0 && (
          <p className="text-muted-foreground p-2 text-sm">No changes</p>
        )}
        {staged.length > 0 && (
          <Group
            title="Staged"
            files={staged}
            actions={
              !readOnly && <TooltipIconButton
                tooltip="Unstage all"
                size="icon-xs"
                disabled={locked}
                onClick={() => unstage(null)}
              >
                <Minus />
              </TooltipIconButton>
            }
          >
            {staged.map((file) => (
              <FileRow
                key={`staged:${file.path}`}
                file={file}
                onOpen={review ? () => review(file.path, true) : null}
              >
                {!readOnly && <>
                <TooltipIconButton
                  tooltip="Discard"
                  size="icon-xs"
                  disabled={locked}
                  onClick={() => setDiscarding({ scope: "staged", files: [file] })}
                >
                  <Undo />
                </TooltipIconButton>
                <TooltipIconButton
                  tooltip="Unstage"
                  size="icon-xs"
                  disabled={locked}
                  onClick={() => unstage([file.path])}
                >
                  <Minus />
                </TooltipIconButton>
                </>}
              </FileRow>
            ))}
          </Group>
        )}
        {changes.length > 0 && (
          <Group
            title="Changes"
            files={changes}
            actions={
              !readOnly && <TooltipIconButton
                tooltip="Stage all"
                size="icon-xs"
                disabled={locked}
                onClick={() => stage(null)}
              >
                <Plus />
              </TooltipIconButton>
            }
          >
            {changes.map((file) => (
              <FileRow
                key={`changes:${file.path}`}
                file={file}
                onOpen={review ? () => review(file.path, false) : null}
              >
                {!readOnly && <>
                <TooltipIconButton
                  tooltip="Discard"
                  size="icon-xs"
                  disabled={locked}
                  onClick={() => setDiscarding({ scope: "unstaged", files: [file] })}
                >
                  <Undo />
                </TooltipIconButton>
                <TooltipIconButton
                  tooltip="Stage"
                  size="icon-xs"
                  disabled={locked || file.status === "conflicted"}
                  onClick={() => stage([file.path])}
                >
                  <Plus />
                </TooltipIconButton>
                </>}
              </FileRow>
            ))}
          </Group>
        )}
      </div>
      <DiscardDialog
        discarding={discarding}
        unstagedPaths={unstagedPaths}
        onCancel={() => setDiscarding(null)}
        onConfirm={() => {
          if (discarding) discard(discarding, false);
          setDiscarding(null);
        }}
      />
    </div>
  );
}

/** A group of files under its title, its count and its actions. */
const Group: FC<{ title: string; files: SourceFile[]; actions: ReactNode; children: ReactNode }> = ({
  title,
  files,
  actions,
  children,
}) => (
  <section aria-label={title} className="mt-2">
    <header className="group/source-group text-muted-foreground flex h-control-sm items-center gap-1 px-2 text-xs font-medium">
      <span className="flex-1">{title}</span>
      <span className="opacity-0 transition-opacity group-hover/source-group:opacity-100 group-focus-within/source-group:opacity-100">
        {actions}
      </span>
      <span className="bg-muted rounded-full px-1.5 tabular-nums">{files.length}</span>
    </header>
    <ul>{children}</ul>
  </section>
);

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
      <span className={cn("shrink-0", file.status === "deleted" && "line-through")}>
        {baseName(file.path)}
      </span>
      <span className="text-muted-foreground min-w-0 truncate text-xs">{dirName(file.path)}</span>
    </>
  );
  const title = file.oldPath ? `${file.oldPath} → ${file.path}` : file.path;
  const look = "flex min-w-0 flex-1 items-center gap-1.5 rounded-sm ps-2 text-start text-sm";
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
  return (
    <Dialog open={discarding !== null} onOpenChange={(open) => !open && onCancel()}>
      <DialogContent>
        {discarding && file && (
          <>
            <DialogHeader>
              <DialogTitle>
                {discarding.scope === "unstaged" && file.status === "untracked"
                  ? `Delete ${baseName(file.path)}?`
                  : `Discard ${discarding.scope === "staged" ? "staged " : ""}changes to ${baseName(file.path)}?`}
              </DialogTitle>
              <DialogDescription>
                {discardWords(
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
                {discarding.scope === "unstaged" && file.status === "untracked" ? "Delete" : "Discard"}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
};

type Busy = null | "commit" | "commitAndPush";

/**
 * The branch, the message box (blank: written for you), "Leave out AI co-author lines" (the
 * Settings › Git switch), Commit ⌘⏎, Commit & push when the branch has a remote, and Review all.
 */
const CommitBox: FC<{
  target: SourceTarget;
  state: SourceState | null;
  /** A stage, unstage or discard is under way. */
  staging: boolean;
  onCommitting: (committing: boolean) => void;
  onCommitted: () => void;
}> = ({ target, state, staging, onCommitting, onCommitted }) => {
  const conversationId = sessionOf(target);
  const key = targetKey(target);
  const [message, setMessageState] = useState(() => unsent.get(key) ?? "");
  const setMessage = (next: string) => {
    if (next) unsent.set(key, next);
    else unsent.delete(key);
    setMessageState(next);
  };
  const [busy, setBusyState] = useState<Busy>(null);
  const setBusy = (next: Busy) => {
    setBusyState(next);
    onCommitting(next !== null);
  };
  const omit = useApp((s) => s.settings.omitAiCoauthors);
  const mac = useApp((s) => s.info?.platform === "macos");
  const nothing = !state || state.staged.length === 0;
  const commit = (push: boolean) => {
    if (nothing || busy || staging) return;
    setBusy(push ? "commitAndPush" : "commit");
    sourceCalls(target)
      .commit(message.trim() || null, push)
      .then(({ outcome }) => {
        const done = outcome.pushed
          ? `Committed and pushed to ${outcome.branch}`
          : `Committed to ${outcome.branch}`;
        // Why the chosen writer model didn't write the message, when it didn't.
        toast(outcome.note ? `${done}. ${outcome.note}` : done);
        setMessage("");
      })
      .catch((failure: unknown) => toast(`Failed to commit: ${reason(failure)}`, { tone: "error" }))
      .finally(() => {
        setBusy(null);
        onCommitted();
      });
  };
  const glyph = (kind: Busy, icon: ReactNode) =>
    busy === kind ? <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" /> : icon;
  return (
    <div className="border-border flex shrink-0 flex-col gap-2 border-b p-2">
      <div className="text-muted-foreground flex h-control-sm items-center gap-1.5 px-1 text-sm">
        <Branch className="size-icon-sm shrink-0" />
        <span className="text-foreground min-w-0 truncate">{state?.branch ?? "…"}</span>
        {state && state.ahead > 0 && (
          <span title={`${state.ahead} not pushed`} className="flex shrink-0 items-center text-xs tabular-nums">
            <ArrowUp className="size-icon-xs" />
            {state.ahead}
          </span>
        )}
        <span className="flex-1" />
        {conversationId && (
          <Button
            variant="ghost"
            size="xs"
            onClick={() => {
              setReviewScope(conversationId, { type: "uncommitted" });
              openReviewTab(conversationId, { type: "all" });
            }}
          >
            Review all
          </Button>
        )}
      </div>
      <textarea
        rows={3}
        value={message}
        disabled={busy !== null}
        aria-label="Commit message"
        placeholder="Message (leave blank to generate)"
        onChange={(event) => setMessage(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (mac ? event.metaKey : event.ctrlKey)) {
            event.preventDefault();
            commit(false);
          }
        }}
        className="border-border rounded-control placeholder:text-muted-foreground resize-none border bg-transparent px-2 py-1.5 text-sm"
      />
      <label className="flex cursor-pointer items-center gap-2 px-1 text-sm">
        <input
          type="checkbox"
          checked={omit}
          onChange={(event) =>
            void setSetting("omitAiCoauthors", event.target.checked).catch((cause: unknown) =>
              toast(reason(cause), { tone: "error" }),
            )
          }
          className="peer sr-only"
        />
        <span
          aria-hidden
          className="border-border peer-checked:bg-foreground/10 peer-focus-visible:ring-ring/50 rounded-xs flex size-icon-sm items-center justify-center border peer-focus-visible:ring-1"
        >
          {omit && <Check className="size-icon-xs" />}
        </span>
        <span className="text-muted-foreground">Leave out AI co-author lines</span>
      </label>
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          className="flex-1"
          disabled={nothing || busy !== null || staging}
          title={nothing ? "Stage changes to commit them" : undefined}
          onClick={() => commit(false)}
        >
          {glyph("commit", <Commit />)}
          {busy === "commit" && !message.trim() ? "Writing message…" : "Commit"}
          <Kbd>{mac ? "⌘⏎" : "Ctrl+⏎"}</Kbd>
        </Button>
        {state?.remote && (
          <Button
            size="sm"
            variant="secondary"
            className="flex-1"
            disabled={nothing || busy !== null || staging}
            onClick={() => commit(true)}
          >
            {glyph("commitAndPush", <UploadDocuments />)}
            {busy === "commitAndPush" && !message.trim() ? "Writing message…" : "Commit & push"}
          </Button>
        )}
      </div>
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
