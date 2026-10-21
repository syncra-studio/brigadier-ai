import { useCallback, useEffect, useMemo, useState } from "react";

import { FileBrowser } from "@/app/side-panel/FileBrowser";
import { listFiles, listProjectFiles } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openFileTab, replaceNewTabWithFile } from "@/state/sessionTabs";
import {
  type FileList, onWorkspaceChanged, readFiles, useCachedEntry, useCheckoutKey, type WorkspaceTarget,
} from "@/state/workspaceCache";

/**
 * The Files panel (⌘P): the session checkout's files as a tree with a search field. A click
 * opens a file in the preview tab (in italic, the next click takes its place); a double click
 * or Enter opens it for good. Before a session starts it lists its project folder's files.
 * Changed files are coloured with their status letter, as Source marks them, and a folder
 * holding changes gets a dot; ignored entries are dimmed and read only when opened.
 */

/** The side panel's Files tab: the tree. */
export function FilesTab({ conversationId, searchRequest = 0, onSearchHandled, replaceTabId, onFileOpened }: {
  conversationId: string;
  searchRequest?: number;
  onSearchHandled?: (() => void) | undefined;
  replaceTabId?: string | undefined;
  onFileOpened?: (() => void) | undefined;
}) {
  const target = useMemo(() => ({ type: "conversation", conversationId }) as const, [conversationId]);
  // Workers landing change the checkout.
  const landed = useBoard((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.tasks).filter((task) => task.landed !== null).length : 0);
  const list = useWorkspaceFiles(target, landed);
  const search = useCallback((query: string) => listFiles(conversationId, query), [conversationId]);
  return (
    <FileBrowser
      scope={conversationId}
      target={target}
      list={list}
      search={search}
      searchRequest={searchRequest}
      onSearchHandled={onSearchHandled}
      onOpen={(path, keep) => {
        if (!replaceTabId || !replaceNewTabWithFile(conversationId, replaceTabId, path))
          openFileTab(conversationId, path, { preview: !keep });
        onFileOpened?.();
      }}
    />
  );
}

/**
 * The Files pane before a session starts: its project folder's files. There are no tabs yet, so
 * opening a file is up to `onOpen`.
 */
export function ProjectFiles({ projectId, searchRequest, onSearchHandled, onOpen }: {
  projectId: string;
  searchRequest: number;
  onSearchHandled: () => void;
  onOpen: (path: string) => void;
}) {
  const target = useMemo(() => ({ type: "project", projectId }) as const, [projectId]);
  const list = useWorkspaceFiles(target, 0);
  const search = useCallback((query: string) => listProjectFiles(projectId, query), [projectId]);
  return (
    <FileBrowser
      scope={`project:${projectId}`}
      target={target}
      list={list}
      search={search}
      searchRequest={searchRequest}
      onSearchHandled={onSearchHandled}
      onOpen={(path) => onOpen(path)}
    />
  );
}

/**
 * A checkout's files: what was last read of it at once (a prefetch usually has them before the
 * sidebar opens), read again when shown, when the checkout or `changed` changes and when the
 * window comes back.
 */
function useWorkspaceFiles(target: WorkspaceTarget, changed: number): FileList | null {
  const key = useCheckoutKey(target);
  const kept = useCachedEntry(key)?.files;
  const [read, setRead] = useState<{ key: string; list: FileList } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const load = () =>
      readFiles(target).then((list) => { if (live) setRead({ key, list }); }).catch((error: unknown) => {
        console.error("listing the checkout's files failed", error);
        // Keep the last good list when a refresh fails; with none, show an empty one.
        if (live) setFailed(key);
      });
    void load();
    const unsubscribe = onWorkspaceChanged(target, () => void load());
    window.addEventListener("focus", load);
    return () => {
      live = false;
      unsubscribe();
      window.removeEventListener("focus", load);
    };
    // `changed` asks for a fresh read even though nothing read here depends on it.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [target, key, changed]);
  return kept ?? (read?.key === key ? read.list : failed === key ? EMPTY : null);
}

const EMPTY: FileList = { files: [], truncated: false };
