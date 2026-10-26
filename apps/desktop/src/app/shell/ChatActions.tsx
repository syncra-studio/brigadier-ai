import { useAui, useThreadListSource } from "@/components/conversation";
import { type FC, useEffect } from "react";

import { FORK_PLACES } from "@/app/conversation/ForkMenu";
import { useLatestAnswer } from "@/app/conversation/SlashCommands";
import { NameDialog } from "@/app/shell/NameDialog";
import { TooltipIconButton } from "@/components/controls";
import {
  Archive,
  BranchAlt,
  Copy,
  Dock,
  DotsHorizontal,
  Folder,
  Pencil,
  Trash,
} from "@/components/icons";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { RowAction } from "@/components/shared";
import { openFolder } from "@/ipc/client";
import type { Conversation, ForkPlace } from "@/ipc/generated";
import {
  archive,
  forkConversation,
  renameConversation,
  setPinned,
} from "@/state/actions";
import { askDelete } from "@/state/picking";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** Where a session works: its worktree once created, else the repository. Chats work nowhere. */
function workingDirectory(conversation: Conversation): string | null {
  const setup = conversation.setup;
  if (setup?.type !== "session") return null;
  return (setup.environment.type === "newWorktree" && setup.environment.path) || setup.repo;
}

/** The thread on its shown branch as Markdown, for "Copy as Markdown". */
function useThreadMarkdown(title: string): () => string {
  const aui = useAui();
  // The whole branch: assistant-ui holds only some of a long one.
  const list = useThreadListSource();
  return () => {
    const parts = [`# ${title}`];
    for (const message of list?.get().messages ?? aui.thread().getState().messages) {
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
 * The ⋯ "Session actions" ("Chat actions" in a Chat) of the header, the tab and the sidebar's
 * rows: Rename ⌥⌘R; Copy › (a session's path, its id, Markdown); Fork ›; Open in › (a session's
 * path); Archive ⇧⌘A and Permanently delete. ⌥⌘P pins it. Share, side chats, scheduled tasks and new windows need what Brigadier doesn't have,
 * so they are not offered.
 */
export const ChatActions: FC<{
  conversation: Conversation;
  onRename: () => void;
  /** Its ⋯ as a small button inside a tab. */
  compact?: boolean;
  /**
   * Its ⋯ as a sidebar row's hover action. No thread is around a row, so Fork and Copy as
   * Markdown aren't offered there, and the conversation's shortcuts stay with its open view.
   */
  row?: boolean;
  className?: string;
}> = ({ conversation, onRename, compact = false, row = false, className }) => {
  const id = conversation.id;
  const pinned = conversation.pinnedAtMs !== null;
  const archived = conversation.lifecycle === "archived";
  const directory = workingDirectory(conversation);
  const noun = conversation.kind === "chat" ? "chat" : "session";
  const actionsName = conversation.kind === "chat" ? "Chat actions" : "Session actions";
  const mac = useApp((s) => s.info?.platform === "macos");

  const togglePin = () => void setPinned(id, !pinned).catch(fail);
  // No confirmation, even while it runs: archiving stops it, and Undo brings it back stopped.
  const archiveIt = () => void archive(id).catch(fail);

  // ⌥⌘R and ⌥⌘P (Ctrl on Windows and Linux) while this conversation is open. ⇧⌘A is the
  // app's (useLifecycleShortcuts).
  useEffect(() => {
    if (archived || row) return;
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
    <DropdownMenu modal={false}>
      {row ? (
        // A row's hover action, as the project row's ⋯ is.
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <RowAction aria-label={`Actions for ${conversation.title}`} className={className}>
                <DotsHorizontal />
              </RowAction>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="bottom">{actionsName}</TooltipContent>
        </Tooltip>
      ) : (
        <DropdownMenuTrigger asChild>
          <TooltipIconButton
            look={compact ? "ghost" : "toolbar"}
            tooltip={actionsName}
            size={compact ? "icon-xs" : "icon-md"}
            className={className}
          >
            <DotsHorizontal />
          </TooltipIconButton>
        </DropdownMenuTrigger>
      )}
      <DropdownMenuContent align={row ? "start" : "end"}>
        {!archived && (
          <>
            <DropdownMenuItem onSelect={onRename}>
              <Pencil />
              Rename
              <DropdownMenuShortcut>{key("⌥⌘R")}</DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
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
                  void copy(directory, "Copied path").catch(() =>
                    toast("Failed to copy path", { tone: "error" }),
                  )
                }
              >
                Copy path
              </DropdownMenuItem>
            )}
            <DropdownMenuItem
              onSelect={() =>
                void copy(shortId(id), `Copied ${noun} id`).catch(() =>
                  toast(`Failed to copy ${noun} id`, { tone: "error" }),
                )
              }
            >
              Copy {noun} id
            </DropdownMenuItem>
            {!row && <CopyMarkdownItem title={conversation.title} />}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        {!row && !archived && <ForkSub conversation={conversation} />}
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
        <DropdownMenuSeparator />
        {!archived && (
          <DropdownMenuItem onSelect={archiveIt}>
            <Archive />
            Archive
            <DropdownMenuShortcut>{key("⇧⌘A")}</DropdownMenuShortcut>
          </DropdownMenuItem>
        )}
        {/* The Archived page's confirm, for a conversation still at work too: deleting stops it. */}
        <DropdownMenuItem variant="destructive" onSelect={() => askDelete([id])}>
          <Trash />
          Permanently delete
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

/** The id the `brigadier` CLI names a conversation by: the last 8 characters of its id (ConversationId::short). */
function shortId(id: string): string {
  return id.slice(-8);
}

/** Copy › Copy as Markdown: the thread around, as it shows. */
const CopyMarkdownItem: FC<{ title: string }> = ({ title }) => {
  const markdown = useThreadMarkdown(title);
  return (
    <DropdownMenuItem
      onSelect={() =>
        void copy(markdown(), "Copied conversation as Markdown").catch(() =>
          toast("Failed to copy conversation as Markdown", { tone: "error" }),
        )
      }
    >
      Copy as Markdown
    </DropdownMenuItem>
  );
};

/** Fork ›, from the thread's latest answer; none before there is one. */
const ForkSub: FC<{ conversation: Conversation }> = ({ conversation }) => {
  const answer = useLatestAnswer();
  if (!answer) return null;
  const fork = (place: ForkPlace) => void forkConversation(conversation.id, answer, place).catch(fail);
  return (
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
