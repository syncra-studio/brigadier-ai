import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { type ChangeMarks, changeMarks, LETTERS } from "@/app/conversation/sourceStatus";
import { fuzzyMatch } from "@/components/assistant-ui/elements/menuMatch";
import { ChevronDown, ChevronRight, FileIcon } from "@/components/icons";
import type { SourceState, SourceStatus, WorkspaceFolder } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { listFiles, listProjectFiles } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openFileTab, replaceNewTabWithFile } from "@/state/sessionTabs";
import {
  type FileList, onWorkspaceChanged, readFiles, readFolder, readSource, useCachedEntry, useCheckoutKey, type WorkspaceTarget,
} from "@/state/workspaceCache";
import { SettingsSearch } from "@/components/ui/settings-search";

/**
 * The Files panel (⌘P): the session checkout's files as a tree with a search field. A click
 * opens a file in the preview tab (in italic, the next click takes its place); a double click
 * or Enter opens it for good. Before a session starts it lists its project folder's files.
 * Changed files are coloured with their status letter, as Source marks them, and a folder
 * holding changes gets a dot; ignored entries are dimmed and read only when opened.
 */

/** Opens `path`: in the preview tab, or with `keep` in a tab of its own. */
type OnOpen = (path: string, keep: boolean) => void;

/** Search results listed at once; typing more narrows them. */
const RESULTS = 100;
/** How long typing pauses before the daemon searches a checkout too big to list whole. */
const SEARCH_DELAY_MS = 150;

/** The files matching a query in the whole checkout. */
type Search = (query: string) => Promise<FileList>;

/** The folders open in each session's (or project folder's) tree, kept while the app runs. */
const openFolders = new Map<string, Set<string>>();

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function dirName(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

type Row =
  | { kind: "folder"; path: string; name: string; depth: number; open: boolean; ignored: boolean; status: SourceStatus | undefined }
  | { kind: "file"; path: string; name: string; depth: number; ignored: boolean; status: SourceStatus | undefined };

type Folder = { folders: Map<string, Folder>; files: string[]; ignored: boolean };

type Tree = { root: Folder; ignoredFiles: ReadonlySet<string> };

function folderAt(root: Folder, path: string): Folder {
  let folder = root;
  for (const part of path.split("/")) {
    let next = folder.folders.get(part);
    if (!next) {
      next = { folders: new Map(), files: [], ignored: false };
      folder.folders.set(part, next);
    }
    folder = next;
  }
  return folder;
}

/**
 * The tree of a checkout's files, the ones deleted from the index too (still shown, struck
 * through), with what it ignores: an ignored folder's contents once it was read.
 */
function buildTree(list: FileList | null, marks: ChangeMarks, loaded: ReadonlyMap<string, WorkspaceFolder>): Tree {
  const root: Folder = { folders: new Map(), files: [], ignored: false };
  const listed = new Set(list?.files);
  const add = (path: string) => {
    const slash = path.lastIndexOf("/");
    (slash < 0 ? root : folderAt(root, path.slice(0, slash))).files.push(path);
  };
  for (const path of list?.files ?? []) add(path);
  for (const [path, status] of marks.files) if (status === "deleted" && !listed.has(path)) add(path);
  const ignoredFiles = new Set<string>();
  for (const entry of list?.ignored ?? []) {
    if (entry.endsWith("/")) folderAt(root, entry.slice(0, -1)).ignored = true;
    else {
      ignoredFiles.add(entry);
      add(entry);
    }
  }
  // Read in order, so a folder is there before what it holds.
  for (const [path, folder] of [...loaded].toSorted(([a], [b]) => a.length - b.length)) {
    const node = folderAt(root, path);
    for (const entry of folder.entries) {
      const child = `${path}/${entry.name}`;
      if (entry.folder) folderAt(root, child).ignored = true;
      else if (!node.files.includes(child)) {
        ignoredFiles.add(child);
        node.files.push(child);
      }
    }
  }
  return { root, ignoredFiles };
}

/**
 * The tree's visible rows: folders first, then files, each by name; open folders expanded.
 * What an ignored folder holds is ignored too.
 */
function visibleRows({ root, ignoredFiles }: Tree, open: ReadonlySet<string>, marks: ChangeMarks): Row[] {
  const rows: Row[] = [];
  const walk = (folder: Folder, prefix: string, depth: number, ignoredAbove: boolean) => {
    const names = [...folder.folders.keys()].toSorted((a, b) => a.localeCompare(b));
    for (const name of names) {
      const path = prefix ? `${prefix}/${name}` : name;
      const child = folder.folders.get(name)!;
      const ignored = ignoredAbove || child.ignored;
      const isOpen = open.has(path);
      rows.push({ kind: "folder", path, name, depth, open: isOpen, ignored, status: ignored ? undefined : marks.folders.get(path) });
      if (isOpen) walk(child, path, depth + 1, ignored);
    }
    const files = folder.files.toSorted((a, b) => baseName(a).localeCompare(baseName(b)));
    for (const path of files) {
      const ignored = ignoredAbove || ignoredFiles.has(path);
      rows.push({ kind: "file", path, name: baseName(path), depth, ignored, status: ignored ? undefined : marks.files.get(path) });
    }
  };
  walk(root, "", 0, false);
  return rows;
}

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

/** A checkout's source state, for the tree's marks: as Source keeps it, read again as it changes. */
function useWorkspaceSource(target: WorkspaceTarget): SourceState | null {
  const key = useCheckoutKey(target);
  const kept = useCachedEntry(key)?.source;
  const [read, setRead] = useState<{ key: string; state: SourceState } | null>(null);
  useEffect(() => {
    let live = true;
    const load = () =>
      readSource(target).then((state) => { if (live) setRead({ key, state }); }).catch((error: unknown) => {
        console.error("reading the checkout's changes failed", error);
      });
    void load();
    const unsubscribe = onWorkspaceChanged(target, () => void load());
    window.addEventListener("focus", load);
    return () => {
      live = false;
      unsubscribe();
      window.removeEventListener("focus", load);
    };
  }, [target, key]);
  return kept ?? (read?.key === key ? read.state : null);
}

const NO_FOLDERS: ReadonlyMap<string, WorkspaceFolder> = new Map();

/**
 * What the ignored folders open in the tree hold, read from disk: when one is shown open (also
 * open again when the tree comes back) and again when the checkout changes. What was read of
 * one checkout is dropped when the target moves to another, late answers too.
 */
function useIgnoredFolders(target: WorkspaceTarget) {
  const key = useCheckoutKey(target);
  const [state, setState] = useState<{ key: string; loaded: ReadonlyMap<string, WorkspaceFolder> }>(
    () => ({ key, loaded: NO_FOLDERS }),
  );
  const loaded = state.key === key ? state.loaded : NO_FOLDERS;
  // The checkout shown now, and the folders asked of it (a failed read waits for a change).
  const current = useRef(key);
  const asked = useRef({ key, paths: new Set<string>() });
  useLayoutEffect(() => {
    current.current = key;
    if (asked.current.key !== key) asked.current = { key, paths: new Set() };
  }, [key]);
  const read = useCallback((path: string) => {
    asked.current.paths.add(path);
    const update = (change: (folders: Map<string, WorkspaceFolder>) => void) =>
      setState((last) => {
        if (current.current !== key) return last;
        const folders = new Map(last.key === key ? last.loaded : NO_FOLDERS);
        change(folders);
        return { key, loaded: folders };
      });
    readFolder(target, path)
      .then((folder) => update((folders) => folders.set(path, folder)))
      .catch((error: unknown) => {
        update((folders) => folders.delete(path));
        console.error("reading an ignored folder failed", error);
      });
  }, [target, key]);
  // Reads the open folders not asked of this checkout yet.
  const want = useCallback((paths: readonly string[]) => {
    if (asked.current.key !== key) return;
    for (const path of paths) if (!asked.current.paths.has(path)) read(path);
  }, [key, read]);
  useEffect(() => onWorkspaceChanged(target, () => {
    if (asked.current.key === key) for (const path of asked.current.paths) read(path);
  }), [target, key, read]);
  return { loaded, want };
}

function FileBrowser({
  scope,
  target,
  list,
  search: searchFiles,
  onOpen,
  searchRequest,
  onSearchHandled,
}: {
  /** Whose files: what the open folders are remembered under. */
  scope: string;
  target: WorkspaceTarget;
  list: FileList | null;
  search: Search;
  onOpen: OnOpen;
  searchRequest: number;
  onSearchHandled?: (() => void) | undefined;
}) {
  const search = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (searchRequest > 0) {
      search.current?.focus({ preventScroll: true });
      onSearchHandled?.();
    }
  }, [searchRequest, onSearchHandled]);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<ReadonlySet<string>>(
    () => openFolders.get(scope) ?? new Set(),
  );
  const [active, setActive] = useState(0);
  const source = useWorkspaceSource(target);
  const marks = useMemo(() => changeMarks(source), [source]);
  const { loaded, want: readIgnored } = useIgnoredFolders(target);
  const tree = useMemo(() => buildTree(list, marks, loaded), [list, marks, loaded]);
  const rows = useMemo(() => visibleRows(tree, open, marks), [tree, open, marks]);
  // Git doesn't list what an ignored folder holds: the open ones shown are read from disk.
  const openIgnored = rows.flatMap((row) => row.kind === "folder" && row.open && row.ignored ? [row.path] : []).join("\n");
  useEffect(() => readIgnored(openIgnored ? openIgnored.split("\n") : []), [openIgnored, readIgnored]);
  const searched = useSearchedFiles(scope, searchFiles, list?.truncated ?? false, query.trim());
  const results = useMemo(() => {
    const want = query.trim();
    if (!want || !list) return null;
    return (searched ?? list.files)
      .flatMap((path) => {
        // The name counts first; a match only in its folders comes after.
        const byName = fuzzyMatch(baseName(path), want);
        const byPath = byName ? null : fuzzyMatch(path, want);
        const match = byName ?? byPath;
        return match ? [{ path, rank: match.rank + (byName ? 0 : 3) }] : [];
      })
      .toSorted((a, b) => a.rank - b.rank || a.path.length - b.path.length)
      .slice(0, RESULTS)
      .map((result) => result.path);
  }, [list, searched, query]);

  const toggle = (path: string) => {
    const next = new Set(open);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    openFolders.set(scope, next);
    setOpen(next);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-border shrink-0 border-b p-2">
        <SettingsSearch
          ref={search}
          variant="bordered"
          label="Search files"
          value={query}
          onChange={(value) => {
            setQuery(value);
            setActive(0);
          }}
          clearable={false}
          clearOnEscape={false}
          onKeyDown={(event) => {
            if (!results) return;
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              const step = event.key === "ArrowDown" ? 1 : -1;
              setActive((index) => Math.min(results.length - 1, Math.max(0, index + step)));
            } else if (event.key === "Enter") {
              const path = results[active];
              if (path) onOpen(path, true);
            }
          }}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {!list ? null : list.files.length === 0 ? (
          <p className="text-muted-foreground p-2 text-sm">No files</p>
        ) : results ? (
          results.length === 0 ? (
            <p className="text-muted-foreground p-2 text-sm">No matching files</p>
          ) : (
            <ul aria-label="Matching files">
              {results.map((path, index) => (
                <li key={path}>
                  <button
                    type="button"
                    title={path}
                    data-active={index === active || undefined}
                    onClick={() => onOpen(path, false)}
                    onDoubleClick={() => onOpen(path, true)}
                    onPointerMove={() => setActive(index)}
                    className="data-active:bg-muted rounded-control flex h-control-md w-full items-center gap-2 px-2 text-start text-sm"
                  >
                    <FileIcon name={path} />
                    <span className="shrink-0">{baseName(path)}</span>
                    <span className="text-muted-foreground min-w-0 truncate">{dirName(path)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )
        ) : (
          <ul aria-label="Files" role="tree">
            {rows.map((row) => (
              <TreeRow
                key={`${row.kind}:${row.path}`}
                row={row}
                onToggle={toggle}
                onOpen={onOpen}
              />
            ))}
          </ul>
        )}
        {list?.truncated && !results && (
          <p className="text-muted-foreground p-2 text-xs">
            Only the first {list.files.length.toLocaleString()} files are listed. Search to find
            the others.
          </p>
        )}
      </div>
    </div>
  );
}

/**
 * When the checkout has more files than the list holds, the files matching `query` from the
 * whole checkout, searched by the daemon once typing pauses; null otherwise, or until then.
 */
function useSearchedFiles(scope: string, search: Search, truncated: boolean, query: string): string[] | null {
  const [found, setFound] = useState<{ scope: string; query: string; files: string[] } | null>(
    null,
  );
  useEffect(() => {
    if (!truncated || !query) return;
    let live = true;
    const timer = setTimeout(() => {
      search(query)
        .then(({ files }) => live && setFound({ scope, query, files }))
        .catch(() => undefined);
    }, SEARCH_DELAY_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [scope, search, truncated, query]);
  return truncated && found?.scope === scope && found.query === query
    ? found.files
    : null;
}

const TreeRow = memo(function TreeRow({
  row,
  onToggle,
  onOpen,
}: {
  row: Row;
  onToggle: (path: string) => void;
  onOpen: OnOpen;
}) {
  const folder = row.kind === "folder";
  const mark = row.status ? LETTERS[row.status] : null;
  return (
    <li role="treeitem" aria-expanded={folder ? row.open : undefined} aria-selected={false}>
      <button
        type="button"
        title={mark ? `${row.path} · ${mark.label}` : row.ignored ? `${row.path} · Ignored` : row.path}
        onClick={() => (folder ? onToggle(row.path) : onOpen(row.path, false))}
        onDoubleClick={() => !folder && onOpen(row.path, true)}
        // Each level indents by one step of the spacing scale.
        style={{ paddingInlineStart: `calc(var(--spacing) * ${2 + row.depth * 4})` }}
        className={cn(
          "hover:bg-muted rounded-control flex h-control-sm w-full items-center gap-1.5 pe-2 text-start text-sm",
          mark ? mark.className : "text-foreground",
          row.ignored && "opacity-50",
        )}
      >
        {folder ? (
          row.open ? <ChevronDown className="size-icon-xs shrink-0" /> : <ChevronRight className="size-icon-xs shrink-0" />
        ) : (
          <FileIcon name={row.path} className="size-icon-xs" />
        )}
        <span className={cn("min-w-0 flex-1 truncate", !folder && row.status === "deleted" && "line-through")}>{row.name}</span>
        {mark && (folder ? (
          <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-current" />
        ) : (
          <span aria-hidden className="w-3 shrink-0 text-center text-xs">{mark.letter}</span>
        ))}
      </button>
    </li>
  );
});
