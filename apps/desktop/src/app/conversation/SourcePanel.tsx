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
import { type FC, type ReactNode, useCallback, useEffect, useState } from "react";

import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
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
import type { SourceFile, SourceScope, SourceState, SourceStatus } from "@/ipc/generated";
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
 */

/** How often the panel reads the checkout again while it shows: agents change files meanwhile. */
const REFRESH_MS = 4000;

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

const LETTERS: Record<SourceStatus, { letter: string; label: string; className: string }> = {
  modified: { letter: "M", label: "Modified", className: "text-warning" },
  added: { letter: "A", label: "Added", className: "text-success" },
  deleted: { letter: "D", label: "Deleted", className: "text-destructive" },
  renamed: { letter: "R", label: "Renamed", className: "text-link" },
  copied: { letter: "C", label: "Copied", className: "text-link" },
  typeChanged: { letter: "T", label: "Type changed", className: "text-warning" },
  untracked: { letter: "U", label: "Untracked", className: "text-success" },
  conflicted: { letter: "!", label: "Conflicted", className: "text-destructive" },
};

/** The checkout's state, read now, again while it shows, and as the board changes it. */
function useSourceState(conversationId: string) {
  const [read, setRead] = useState<{ state: SourceState | null; error: string | null }>({
    state: null,
    error: null,
  });
  const changeKey = useCheckoutChanges(conversationId);
  const load = useCallback(() => {
    request({ method: "getSourceState", conversationId })
      .then(({ state }) => {
        setRead({ state, error: null });
        noteSourceState(conversationId, state);
      })
      .catch((error: unknown) => setRead((current) => ({ state: current.state, error: reason(error) })));
  }, [conversationId]);
  // A landing, Undo or Reapply changes the checkout at once.
  useEffect(() => {
    if (changeKey) load();
  }, [changeKey, load]);
  useEffect(() => {
    load();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") load();
    }, REFRESH_MS);
    window.addEventListener("focus", load);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", load);
    };
  }, [load]);
  const set = (state: SourceState) => {
    setRead({ state, error: null });
    noteSourceState(conversationId, state);
  };
  return { ...read, set, load };
}

type Discarding = { scope: SourceScope; files: SourceFile[] };

export function SourcePanel({ conversationId }: { conversationId: string }) {
  const { state, error, set, load } = useSourceState(conversationId);
  const [busy, setBusy] = useState(false);
  // A commit reads the staged diff for its message, then commits the index: it stays as is.
  const [committing, setCommitting] = useState(false);
  const locked = busy || committing;
  const [discarding, setDiscarding] = useState<Discarding | null>(null);
  const review = (path: string, staged: boolean) => {
    openReviewTab(conversationId, { type: "file", path, staged });
  };

  const act = (run: () => Promise<{ state: SourceState }>, failed: string) => {
    setBusy(true);
    run()
      .then(({ state: next }) => set(next))
      .catch((cause: unknown) => toast(`${failed}: ${reason(cause)}`, { tone: "error" }))
      .finally(() => setBusy(false));
  };
  const stage = (paths: string[] | null) =>
    act(() => request({ method: "stageFiles", conversationId, paths }), "Failed to stage");
  const unstage = (paths: string[] | null) =>
    act(() => request({ method: "unstageFiles", conversationId, paths }), "Failed to unstage");
  const discard = ({ scope, files }: Discarding, all: boolean) =>
    act(
      () =>
        request({
          method: "discardFiles",
          conversationId,
          scope,
          paths: all ? null : files.map((file) => file.path),
        }),
      "Failed to discard",
    );

  const staged = state?.staged ?? [];
  const changes = state?.changes ?? [];
  const unstagedPaths = new Set(changes.map((file) => file.path));

  return (
    <div data-slot="source-panel" className="flex min-h-0 flex-1 flex-col">
      <CommitBox
        conversationId={conversationId}
        state={state}
        staging={busy}
        onCommitting={setCommitting}
        onCommitted={load}
      />
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
              <TooltipIconButton
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
                onOpen={() => review(file.path, true)}
              >
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
              </FileRow>
            ))}
          </Group>
        )}
        {changes.length > 0 && (
          <Group
            title="Changes"
            files={changes}
            actions={
              <TooltipIconButton
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
                onOpen={() => review(file.path, false)}
              >
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

/** A changed file: click to open its diff; its actions show on hover. */
const FileRow: FC<{ file: SourceFile; onOpen: () => void; children: ReactNode }> = ({
  file,
  onOpen,
  children,
}) => {
  const status = LETTERS[file.status];
  return (
    <li className="group/source-row hover:bg-muted rounded-control flex h-control-sm items-center gap-1 pe-1">
      <button
        type="button"
        title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
        onClick={onOpen}
        className="flex min-w-0 flex-1 items-center gap-1.5 rounded-sm ps-2 text-start text-sm"
      >
        <FileTypeIcon name={file.path} className="size-icon-sm shrink-0" />
        <span className={cn("shrink-0", file.status === "deleted" && "line-through")}>
          {baseName(file.path)}
        </span>
        <span className="text-muted-foreground min-w-0 truncate text-xs">{dirName(file.path)}</span>
      </button>
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
  conversationId: string;
  state: SourceState | null;
  /** A stage, unstage or discard is under way. */
  staging: boolean;
  onCommitting: (committing: boolean) => void;
  onCommitted: () => void;
}> = ({ conversationId, state, staging, onCommitting, onCommitted }) => {
  const [message, setMessage] = useState("");
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
    request({
      method: "commitChanges",
      conversationId,
      message: message.trim() || null,
      includeUnstaged: false,
      push,
    })
      .then(({ outcome }) => {
        toast(
          outcome.pushed
            ? `Committed and pushed to ${outcome.branch}`
            : `Committed to ${outcome.branch}`,
        );
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
