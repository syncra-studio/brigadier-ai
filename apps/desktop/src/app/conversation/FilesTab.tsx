import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  Search,
} from "@openai/apps-sdk-ui/components/Icon";
import { memo, useEffect, useMemo, useRef, useState } from "react";

import { useCheckoutFiles } from "@/app/conversation/Mentions";
import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { fuzzyMatch } from "@/components/assistant-ui/elements/fuzzy-match";
import { listFiles } from "@/state/actions";
import { openFileTab, replaceNewTabWithFile } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/**
 * The Files panel (⌘P): the session checkout's files as a tree with a search field. A click
 * opens a file in the preview tab (in italic, the next click takes its place); a double click
 * or Enter opens it for good.
 */

/** Opens `path`: in the preview tab, or with `keep` in a tab of its own. */
type OnOpen = (path: string, keep: boolean) => void;

/** Search results listed at once; typing more narrows them. */
const RESULTS = 100;
/** How long typing pauses before the daemon searches a checkout too big to list whole. */
const SEARCH_DELAY_MS = 150;

/** The folders open in each session's tree, kept while the app runs. */
const openFolders = new Map<string, Set<string>>();

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function dirName(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

type Row =
  | { kind: "folder"; path: string; name: string; depth: number; open: boolean }
  | { kind: "file"; path: string; name: string; depth: number };

type Folder = { folders: Map<string, Folder>; files: string[] };

function buildTree(files: readonly string[]): Folder {
  const root: Folder = { folders: new Map(), files: [] };
  for (const path of files) {
    const parts = path.split("/");
    let folder = root;
    for (const part of parts.slice(0, -1)) {
      let next = folder.folders.get(part);
      if (!next) {
        next = { folders: new Map(), files: [] };
        folder.folders.set(part, next);
      }
      folder = next;
    }
    folder.files.push(path);
  }
  return root;
}

/** The tree's visible rows: folders first, then files, each by name; open folders expanded. */
function visibleRows(root: Folder, open: ReadonlySet<string>): Row[] {
  const rows: Row[] = [];
  const walk = (folder: Folder, prefix: string, depth: number) => {
    const names = [...folder.folders.keys()].toSorted((a, b) => a.localeCompare(b));
    for (const name of names) {
      const path = prefix ? `${prefix}/${name}` : name;
      const isOpen = open.has(path);
      rows.push({ kind: "folder", path, name, depth, open: isOpen });
      const child = folder.folders.get(name);
      if (isOpen && child) walk(child, path, depth + 1);
    }
    const files = folder.files.toSorted((a, b) => baseName(a).localeCompare(baseName(b)));
    for (const path of files) rows.push({ kind: "file", path, name: baseName(path), depth });
  };
  walk(root, "", 0);
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
  return (
    <FileBrowser
      conversationId={conversationId}
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

function FileBrowser({
  conversationId,
  onOpen,
  searchRequest,
  onSearchHandled,
}: {
  conversationId: string;
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
  const conversation = useApp((s) => s.conversations[conversationId]);
  const list = useCheckoutFiles(conversation ?? null);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<ReadonlySet<string>>(
    () => openFolders.get(conversationId) ?? new Set(),
  );
  const [active, setActive] = useState(0);
  const tree = useMemo(() => buildTree(list?.files ?? []), [list]);
  const rows = useMemo(() => visibleRows(tree, open), [tree, open]);
  const searched = useSearchedFiles(conversationId, list?.truncated ?? false, query.trim());
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
    openFolders.set(conversationId, next);
    setOpen(next);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-border shrink-0 border-b p-2">
        <label className="border-border rounded-control flex h-control-md items-center gap-1.5 border px-2">
          <Search className="text-muted-foreground size-icon-sm shrink-0" />
          <input
            ref={search}
            value={query}
            placeholder="Search files"
            aria-label="Search files"
            onChange={(event) => {
              setQuery(event.target.value);
              setActive(0);
            }}
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
            className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none"
          />
        </label>
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
                    <FileTypeIcon name={path} className="size-icon-sm shrink-0" />
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
function useSearchedFiles(conversationId: string, truncated: boolean, query: string): string[] | null {
  const [found, setFound] = useState<{ conversationId: string; query: string; files: string[] } | null>(
    null,
  );
  useEffect(() => {
    if (!truncated || !query) return;
    let live = true;
    const timer = setTimeout(() => {
      listFiles(conversationId, query)
        .then(({ files }) => live && setFound({ conversationId, query, files }))
        .catch(() => undefined);
    }, SEARCH_DELAY_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [conversationId, truncated, query]);
  return truncated && found?.conversationId === conversationId && found.query === query
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
  return (
    <li role="treeitem" aria-expanded={folder ? row.open : undefined} aria-selected={false}>
      <button
        type="button"
        title={row.path}
        onClick={() => (folder ? onToggle(row.path) : onOpen(row.path, false))}
        onDoubleClick={() => !folder && onOpen(row.path, true)}
        // Each level indents by one step of the spacing scale.
        style={{ paddingInlineStart: `calc(var(--spacing) * ${2 + row.depth * 4})` }}
        className="hover:bg-muted text-muted-foreground hover:text-foreground rounded-control flex h-control-sm w-full items-center gap-1.5 pe-2 text-start text-sm"
      >
        {folder ? (
          <>
            {row.open ? (
              <ChevronDown className="size-icon-xs shrink-0" />
            ) : (
              <ChevronRight className="size-icon-xs shrink-0" />
            )}
            {row.open ? (
              <FolderOpen className="size-icon-sm shrink-0" />
            ) : (
              <Folder className="size-icon-sm shrink-0" />
            )}
          </>
        ) : (
          <FileTypeIcon name={row.path} className="ms-4 size-icon-sm shrink-0" />
        )}
        <span className="min-w-0 truncate">{row.name}</span>
      </button>
    </li>
  );
});
