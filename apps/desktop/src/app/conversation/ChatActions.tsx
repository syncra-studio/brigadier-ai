import { useAui } from "@assistant-ui/react";
import {
  Archive,
  BranchAlt,
  Copy,
  DotsHorizontal,
  Folder,
  Pencil,
  Pin,
  Terminal,
  Unpin,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useEffect } from "react";

import { FORK_PLACES } from "@/app/conversation/ForkMenu";
import { useLatestAnswer } from "@/app/conversation/SlashCommands";
import { NameDialog } from "@/app/NameDialog";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
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
  forkConversation,
  renameConversation,
  setPinned,
} from "@/state/actions";
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
 * The header's ⋯ "Chat actions": Rename ⌥⌘R, Pin ⌥⌘P, Archive ⇧⌘A; Copy › (working directory,
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
                      <Terminal />
                      Terminal
                    </DropdownMenuItem>
                  )}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </>
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
