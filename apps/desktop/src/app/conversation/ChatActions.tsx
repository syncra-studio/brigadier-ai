import { useAui } from "@assistant-ui/react";
import { type FC, useEffect, useState } from "react";

import { FORK_PLACES } from "@/app/conversation/ForkMenu";
import { useLatestAnswer } from "@/app/conversation/SlashCommands";
import { NameDialog } from "@/app/NameDialog";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  Archive,
  BranchAlt,
  Copy,
  Dock,
  DotsHorizontal,
  Folder,
  Pencil,
  Pin,
  Plus,
  Trash,
  Unpin,
} from "@/components/icons";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { openFolder } from "@/ipc/client";
import type { Conversation, ForkPlace } from "@/ipc/generated";
import {
  archive,
  closeWorkspace,
  forkConversation,
  renameConversation,
  select,
  setPinned,
  setProjectExpanded,
} from "@/state/actions";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** Where a session works: its worktree once created, else the repository. Chats work nowhere. */
function workingDirectory(conversation: Conversation): string | null {
  const setup = conversation.setup;
  if (setup?.type !== "session") return null;
  return (setup.environment.type === "newWorktree" && setup.environment.path) || setup.repo;
}

/** A worktree session's own worktree: what "Remove worktree" removes, and where it merges. */
interface SessionWorkspace {
  path: string;
  branch: string;
  base: string;
}

function sessionWorkspace(conversation: Conversation): SessionWorkspace | null {
  const setup = conversation.setup;
  if (setup?.type !== "session" || setup.environment.type !== "newWorktree" || !setup.environment.path) return null;
  return { path: setup.environment.path, branch: setup.environment.branch, base: setup.environment.base };
}

/** The thread on its shown branch as Markdown, for "Copy as Markdown". */
function useThreadMarkdown(title: string): () => string {
  const aui = useAui();
  return () => {
    const parts = [`# ${title}`];
    for (const message of aui.thread().getState().messages) {
      const text = message.content
        .map((part) => (part.type === "text" ? part.text : ""))
        .filter(Boolean)
        .join("\n\n")
        .trim();
      if (!text) continue;
      parts.push(`## ${message.role === "user" ? "User" : "Assistant"}`, text);
    }
    return `${parts.join("\n\n")}\n`;
  };
}

function fail(cause: unknown) {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

async function copy(text: string, done: string) {
  await navigator.clipboard.writeText(text);
  toast(done);
}

/**
 * The header's ⋯ "Chat actions": Rename ⌥⌘R, Pin ⌥⌘P, Archive ⇧⌘A; a session's New session (in
 * its project) and, with a worktree of its own, Remove worktree…; Copy › (working directory,
 * Markdown); Fork ›; Open in › (a session's working directory). Share, side chats, scheduled
 * tasks and new windows need what Brigadier doesn't have, so they are not offered.
 */
export const ChatActions: FC<{
  conversation: Conversation;
  onRename: () => void;
  /** Its ⋯ as a small button inside a tab. */
  compact?: boolean;
  className?: string;
}> = ({ conversation, onRename, compact = false, className }) => {
  const id = conversation.id;
  const pinned = conversation.pinnedAtMs !== null;
  const archived = conversation.lifecycle === "archived";
  const directory = workingDirectory(conversation);
  const workspace = sessionWorkspace(conversation);
  const projectId = conversation.kind === "session" ? conversation.projectId : null;
  const [closing, setClosing] = useState(false);
  const answer = useLatestAnswer();
  const markdown = useThreadMarkdown(conversation.title);
  const mac = useApp((s) => s.info?.platform === "macos");

  const togglePin = () => void setPinned(id, !pinned).catch(fail);
  // No confirmation, even while it runs: archiving stops it, and Undo brings it back stopped.
  const archiveIt = () => void archive(id).catch(fail);
  const fork = (place: ForkPlace) => {
    if (answer) void forkConversation(id, answer, place).catch(fail);
  };

  // ⌥⌘R and ⌥⌘P (Ctrl on Windows and Linux) while this conversation is open. ⇧⌘A is the
  // app's (useLifecycleShortcuts).
  useEffect(() => {
    if (archived) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(mac ? event.metaKey : event.ctrlKey)) return;
      const action =
        event.altKey && !event.shiftKey && event.code === "KeyR"
          ? onRename
          : event.altKey && !event.shiftKey && event.code === "KeyP"
            ? togglePin
            : null;
      if (!action) return;
      event.preventDefault();
      action();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  const key = (keys: string) =>
    mac ? keys : keys.replace("⌥", "Alt+").replace("⇧", "Shift+").replace("⌘", "Ctrl+");
  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <TooltipIconButton
            look={compact ? "ghost" : "toolbar"}
            tooltip="Chat actions"
            size={compact ? "icon-xs" : "icon-md"}
            className={className}
          >
            <DotsHorizontal />
          </TooltipIconButton>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {!archived && (
            <>
              <DropdownMenuItem onSelect={onRename}>
                <Pencil />
                Rename
                <DropdownMenuShortcut>{key("⌥⌘R")}</DropdownMenuShortcut>
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={togglePin}>
                {pinned ? <Unpin /> : <Pin />}
                {pinned ? "Unpin" : "Pin"}
                <DropdownMenuShortcut>{key("⌥⌘P")}</DropdownMenuShortcut>
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={archiveIt}>
                <Archive />
                Archive
                <DropdownMenuShortcut>{key("⇧⌘A")}</DropdownMenuShortcut>
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {projectId && (
                <DropdownMenuItem
                  onSelect={() => {
                    setProjectExpanded(projectId, true);
                    select({ type: "draft", kind: "session", projectId });
                  }}
                >
                  <Plus />
                  New session
                </DropdownMenuItem>
              )}
              {workspace && (
                <DropdownMenuItem onSelect={() => setClosing(true)}>
                  <Trash />
                  Remove worktree…
                </DropdownMenuItem>
              )}
              {(projectId || workspace) && <DropdownMenuSeparator />}
            </>
          )}
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <Copy />
              Copy
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              {directory && (
                <DropdownMenuItem
                  onSelect={() =>
                    void copy(directory, "Copied working directory").catch(() =>
                      toast("Failed to copy working directory", { tone: "error" }),
                    )
                  }
                >
                  Copy working directory
                </DropdownMenuItem>
              )}
              <DropdownMenuItem
                onSelect={() =>
                  void copy(markdown(), "Copied conversation as Markdown").catch(() =>
                    toast("Failed to copy conversation as Markdown", { tone: "error" }),
                  )
                }
              >
                Copy as Markdown
              </DropdownMenuItem>
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          {answer && !archived && (
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <BranchAlt />
                Fork
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent>
                {conversation.kind === "chat" ? (
                  <DropdownMenuItem onSelect={() => fork("workspace")}>Fork chat</DropdownMenuItem>
                ) : (
                  FORK_PLACES.map(({ place, title }) => (
                    <DropdownMenuItem key={place} onSelect={() => fork(place)}>
                      {title}
                    </DropdownMenuItem>
                  ))
                )}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          )}
          {directory && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>
                  <Folder />
                  Open in
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  <DropdownMenuItem onSelect={() => void openFolder(directory).catch(fail)}>
                    <Folder />
                    {mac ? "Finder" : "File manager"}
                  </DropdownMenuItem>
                  {mac && (
                    <DropdownMenuItem
                      onSelect={() => void openFolder(directory, "Terminal").catch(fail)}
                    >
                      <Dock />
                      Terminal
                    </DropdownMenuItem>
                  )}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      {workspace && (
        <CloseWorkspaceDialog
          conversationId={id}
          workspace={workspace}
          open={closing}
          onOpenChange={setClosing}
        />
      )}
    </>
  );
};

/** The confirm before "Remove worktree": it names the folder that goes and the branch that stays. */
const CloseWorkspaceDialog: FC<{
  conversationId: string;
  workspace: SessionWorkspace;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}> = ({ conversationId, workspace, open, onOpenChange }) => {
  const [busy, setBusy] = useState(false);
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Remove worktree?"
      description={
        <>
          This removes the folder <code className="break-all">{workspace.path}</code>. Uncommitted
          changes are kept as a commit on <code>{workspace.branch}</code>, and the branch stays
          unless all of its work is already on <code>{workspace.base}</code>. The session&apos;s
          next change makes a fresh worktree in the same folder.
        </>
      }
      confirm="Remove worktree"
      destructive
      busy={busy}
      onConfirm={() => {
        setBusy(true);
        closeWorkspace(conversationId)
          .then((text) => {
            toast(text);
            onOpenChange(false);
          })
          .catch((error: unknown) =>
            toast(`Couldn't remove the worktree: ${error instanceof Error ? error.message : String(error)}`, { tone: "error" }),
          )
          .finally(() => setBusy(false));
      }}
    />
  );
};

/** The rename dialog the header's title and ⌥⌘R open. */
export const RenameDialog: FC<{
  conversation: Conversation;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}> = ({ conversation, open, onOpenChange }) => (
  <NameDialog
    open={open}
    onOpenChange={onOpenChange}
    title={conversation.kind === "chat" ? "Rename chat" : "Rename session"}
    label="Title"
    initialValue={conversation.title}
    confirmLabel="Rename"
    onSubmit={(title) => renameConversation(conversation.id, title)}
  />
);
