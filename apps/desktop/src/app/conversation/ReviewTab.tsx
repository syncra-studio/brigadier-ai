import { useAui } from "@assistant-ui/react";
import {
  createContext,
  type FC,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { useCheckoutRoot } from "@/components/assistant-ui/file-links";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Check,
  ChevronDown,
  ChevronDownUp,
  ChevronRight,
  ChevronUpDown,
  Compare,
  Copy,
  ExternalLink,
  FolderOpen,
  Folders,
  Reload,
  Text,
} from "@/components/ui/icon";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { openFolder, request, revealPath } from "@/ipc/client";
import type { ReviewDiff, ReviewFile, ReviewScope } from "@/ipc/generated";
import { type DiffLine, fileSides, filePatch, type PatchFile, parsePatch } from "@/lib/patch";
import { type DiffSource, DiffView } from "@/app/conversation/diffView";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { type ReviewOptions, setReviewOption, useReview } from "@/state/review";
import { CHAT_TAB, openFileTab, type ReviewTarget, reviewScope, selectTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";

/**
 * A session's Review tab, opened from the Source panel: a group's diffs ("Open Changes"), each
 * file's under a sticky header, or one file's staged or unstaged diff. It only reads; a line's
 * "+" adds it to the chat.
 */

const Counts: FC<{ insertions: number; deletions: number; className?: string }> = ({
  insertions,
  deletions,
  className,
}) => (
  <span className={cn("tabular-nums", className)}>
    <span className="text-success">+{insertions}</span>{" "}
    <span className="text-destructive">−{deletions}</span>
  </span>
);

/** Something about the board that changes the diff: a landing, an Undo or Reapply. */
function useChangeKey(conversationId: string): string {
  return useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return "";
    const landed = Object.values(board.tasks)
      .map((task) => task.landed ?? "")
      .join();
    const undone = Object.values(board.requests)
      .map((entry) => entry.undo?.commits.at(-1) ?? "")
      .join();
    return `${landed}|${undone}`;
  });
}

/**
 * The review of `scope` with whole files, refetched when the board, the Source panel's reading
 * of the checkout or `tick` change it. While a refetch loads, the last review stays only if it
 * shows the same scope: another scope's patch must not sit under this one's label.
 */
function useReviewDiff(conversationId: string, scope: ReviewScope, tick: number) {
  const changeKey = useChangeKey(conversationId);
  const checkout = useReview((s) => s.checkouts[conversationId] ?? "");
  const view = JSON.stringify([conversationId, scope]);
  const key = JSON.stringify([view, tick, changeKey, checkout]);
  const [state, setState] = useState<{
    key: string | null;
    view: string | null;
    review: ReviewDiff | null;
    error: string | null;
  }>({ key: null, view: null, review: null, error: null });
  useEffect(() => {
    let live = true;
    const [id, wanted] = JSON.parse(view) as [string, ReviewScope];
    request({
      method: "getReviewDiff",
      conversationId: id,
      scope: wanted,
      wholeFiles: true,
      ignoreWhitespace: false,
    })
      .then(({ review }) => {
        if (live) setState({ key, view, review, error: null });
      })
      .catch((error: unknown) => {
        if (live) {
          setState({
            key,
            view,
            review: null,
            error: error instanceof Error ? error.message : String(error),
          });
        }
      });
    return () => {
      live = false;
    };
  }, [key, view]);
  const same = state.view === view;
  return {
    review: same ? state.review : null,
    error: same ? state.error : null,
    loading: state.key !== key,
  };
}

export function ReviewTab({
  conversationId,
  target,
  active = true,
}: {
  conversationId: string;
  target: ReviewTarget;
  /** In front: it shows changes made meanwhile when it comes to the front again. */
  active?: boolean;
}) {
  // One file's diff: the file's own, or its part of a commit's or a turn's (a Changes card's row).
  const path = target.type === "file" ? target.path : target.type === "commit" || target.type === "lastTurn" ? (target.path ?? null) : null;
  const staged = target.staged;
  const commit = target.type === "commit" ? target.commit : null;
  // The diff is read again only when what it compares changes (it goes by the scope's text).
  const scope = reviewScope(target);
  const options = useReview((s) => s.options);
  const [tick, setTick] = useState(0);
  const [wasActive, setWasActive] = useState(active);
  if (wasActive !== active) {
    setWasActive(active);
    if (active) setTick(tick + 1);
  }
  const { review: fullReview, error, loading } = useReviewDiff(conversationId, scope, tick);
  // One file's diff: only its part of the scope's.
  const review = useMemo(
    () =>
      fullReview && path !== null
        ? (() => {
            const files = fullReview.files.filter((file) => file.path === path);
            return {
              ...fullReview,
              files,
              patch: filePatch(fullReview.patch, path),
              insertions: files.reduce((sum, file) => sum + (file.insertions ?? 0), 0),
              deletions: files.reduce((sum, file) => sum + (file.deletions ?? 0), 0),
            };
          })()
        : fullReview,
    [fullReview, path],
  );
  const patches = useMemo(() => {
    if (!fullReview) return new Map<string, ReviewPatch>();
    const parsed = parsePatch(
      fullReview.patch,
      fullReview.files.map((file) => file.path),
      fullReview.fullFiles,
    );
    return new Map(
      fullReview.files.map((file, index): [string, ReviewPatch] => [
        file.path,
        { ...parsed[index]!, text: filePatch(fullReview.patch, file.path) },
      ]),
    );
  }, [fullReview]);
  const files = useMemo(() => review?.files ?? [], [review]);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const allCollapsed = files.length > 0 && files.every((file) => collapsed.has(file.path));

  return (
    <div data-slot="review-tab" className="flex min-h-0 flex-1 flex-col">
      <div className="border-border flex h-control-lg shrink-0 items-center gap-1 border-b px-2">
        <span className="text-muted-foreground px-1.5 text-sm">
          {commit ? `Merge ${commit.slice(0, 7)}` : target.type === "lastTurn" ? "Last turn" : target.type === "uncommitted" ? "Uncommitted changes" : staged ? "Staged Changes" : "Changes"}
        </span>
        {review && review.files.length > 0 && (
          <Counts insertions={review.insertions} deletions={review.deletions} className="text-sm" />
        )}
        <div className="flex-1" />
        <TooltipIconButton tooltip="Refresh" size="icon-sm" onClick={() => setTick(tick + 1)}>
          <Reload className={cn(loading && "animate-spin motion-reduce:animate-none")} />
        </TooltipIconButton>
        <ToggleButton
          tooltip={options.wrap ? "Disable word wrap" : "Enable word wrap"}
          on={options.wrap}
          onClick={() => setReviewOption("wrap", !options.wrap)}
        >
          <Text />
        </ToggleButton>
        {path === null && (
          <TooltipIconButton
            tooltip={allCollapsed ? "Expand all diffs" : "Collapse all diffs"}
            size="icon-sm"
            disabled={files.length === 0}
            onClick={() =>
              setCollapsed(allCollapsed ? new Set() : new Set(files.map((file) => file.path)))
            }
          >
            {allCollapsed ? <ChevronUpDown /> : <ChevronDownUp />}
          </TooltipIconButton>
        )}
        <ToggleButton
          tooltip={options.split ? "Switch to unified diff" : "Switch to split diff"}
          on={options.split}
          onClick={() => setReviewOption("split", !options.split)}
        >
          <Compare />
        </ToggleButton>
      </div>
      <div className="flex min-h-0 flex-1">
        <ReviewContext.Provider value={conversationId}>
          <div data-slot="review-diffs" className="min-w-0 flex-1 overflow-auto">
            {error ? (
              <p className="text-destructive p-4 text-sm">{error}</p>
            ) : review && review.files.length === 0 ? (
              <p className="text-muted-foreground p-4 text-sm">
                {path !== null ? `No ${staged ? "staged " : ""}changes in ${path}` : "No changes"}
              </p>
            ) : (
              review?.files.map((file) => (
                <FileSection
                  key={file.path}
                  file={file}
                  patch={patches.get(file.path)}
                  options={options}
                  collapsed={collapsed.has(file.path)}
                  onToggle={() =>
                    setCollapsed((current) => {
                      const next = new Set(current);
                      if (next.has(file.path)) next.delete(file.path);
                      else next.add(file.path);
                      return next;
                    })
                  }
                />
              ))
            )}
          </div>
        </ReviewContext.Provider>
      </div>
    </div>
  );
}

const ToggleButton: FC<{
  tooltip: string;
  on: boolean;
  onClick: () => void;
  children: ReactNode;
}> = ({ tooltip, on, onClick, children }) => (
  <TooltipIconButton
    tooltip={tooltip}
    size="icon-sm"
    aria-pressed={on}
    className={cn(on && "bg-muted text-foreground")}
    onClick={onClick}
  >
    {children}
  </TooltipIconButton>
);

/** A file's lines as the Review tab reads them, with its own part of the patch text. */
type ReviewPatch = PatchFile & { text: string };

/** One file's diff under its sticky header: icon, path, counts, and on hover copy and fold. */
const FileSection: FC<{
  file: ReviewFile;
  patch: ReviewPatch | undefined;
  options: ReviewOptions;
  collapsed: boolean;
  onToggle: () => void;
}> = ({ file, patch, options, collapsed, onToggle }) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <section data-slot="review-file" data-path={file.path} className="scroll-mt-0">
      <header className="group/file bg-background border-border sticky top-0 z-10 flex h-control-lg items-center gap-2 border-b px-3 text-sm">
        <FileTypeIcon name={file.path} className="size-icon-sm shrink-0" />
        <span className="text-foreground min-w-0 truncate" title={file.path}>
          {file.from ? `${file.from} → ${file.path}` : file.path}
        </span>
        {file.binary ? (
          <span className="text-muted-foreground text-xs">binary</span>
        ) : (
          <Counts insertions={file.insertions} deletions={file.deletions} />
        )}
        <span className="flex items-center opacity-0 transition-opacity group-hover/file:opacity-100 group-focus-within/file:opacity-100">
          <TooltipIconButton
            tooltip={isCopied ? "Copied" : "Copy path"}
            size="icon-xs"
            onClick={() => copyToClipboard(file.path)}
          >
            {isCopied ? <Check /> : <Copy />}
          </TooltipIconButton>
          <OpenInMenu file={file} />
          <TooltipIconButton
            tooltip="Toggle file diff"
            size="icon-xs"
            aria-expanded={!collapsed}
            onClick={onToggle}
          >
            {collapsed ? <ChevronRight /> : <ChevronDown />}
          </TooltipIconButton>
        </span>
      </header>
      {!collapsed && <FileBody file={file} patch={patch} options={options} />}
    </section>
  );
};

/** Whose checkout a file's diff is of. */
const ReviewContext = createContext("");

function failed(cause: unknown) {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

/** A changed file's "Open in" menu: a file tab, its default app, or the file manager. */
const OpenInMenu: FC<{ file: ReviewFile }> = ({ file }) => {
  const conversationId = useContext(ReviewContext);
  const root = useCheckoutRoot();
  const mac = useApp((s) => s.info?.platform === "macos");
  const gone = file.status === "deleted";
  const absolute = root ? `${root.replace(/\/$/, "")}/${file.path}` : null;
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton tooltip="Open in" size="icon-xs" disabled={gone}>
          <ExternalLink />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={() => openFileTab(conversationId, file.path)}>
          <Folders />
          File tab
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={!absolute}
          onSelect={() => absolute && openFolder(absolute).catch(failed)}
        >
          <ExternalLink />
          Default app
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={!absolute}
          onSelect={() => absolute && revealPath(absolute).catch(failed)}
        >
          <FolderOpen />
          {mac ? "Finder" : "File manager"}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

const FileBody: FC<{ file: ReviewFile; patch: ReviewPatch | undefined; options: ReviewOptions }> = ({
  file,
  patch,
  options,
}) => {
  const addToChat = useAddToChat(file.path);
  // With whole files the engine has both sides, so every unmodified stretch opens in place.
  const source = useMemo<DiffSource | null>(() => {
    if (!patch || patch.binary || patch.rows.length === 0) return null;
    const sides = fileSides(patch.rows);
    return sides ? { kind: "sides", path: file.path, ...sides } : { kind: "patch", patch: patch.text };
  }, [patch, file.path]);
  const onGutterClick = useCallback(
    (at: LineAt) => {
      const line = patch && findLine(patch, at);
      if (line) addToChat(line);
    },
    [patch, addToChat],
  );
  if (!patch || file.binary || patch.binary) {
    return (
      <p className="text-muted-foreground px-3 py-2 text-sm">
        {file.binary || patch?.binary ? "Binary file not shown" : "No content changes"}
      </p>
    );
  }
  if (!source) {
    return <p className="text-muted-foreground px-3 py-2 text-sm">No content changes</p>;
  }
  return (
    <DiffView<null>
      source={source}
      look={{ numbered: true, split: options.split, wrap: options.wrap, words: true }}
      onGutterClick={onGutterClick}
    />
  );
};

/** A line by its number in the new file ("additions") or in the old one. */
type LineAt = { side: "additions" | "deletions"; lineNumber: number };

/** The line at `at`: by its number in the new file, or in the old one for a deleted line. */
function findLine(patch: PatchFile, at: LineAt): DiffLine | null {
  for (const row of patch.rows) {
    for (const line of row.kind === "gap" ? (row.lines ?? []) : [row]) {
      if (at.side === "deletions" ? line.kind !== "add" && line.old === at.lineNumber : line.kind !== "del" && line.new === at.lineNumber) return line;
    }
  }
  return null;
}

/**
 * A line's "+": Add to chat. The line goes into the composer quoted, as text selected in an
 * answer does, to say something about it there. Chat comes to the front to show it.
 */
function useAddToChat(path: string): (line: DiffLine) => void {
  const aui = useAui();
  const conversationId = useContext(ReviewContext);
  return useCallback(
    (line: DiffLine) => {
      const composer = aui.composer();
      const current = composer.getState().text;
      const number = line.kind === "del" ? line.old : line.new;
      const quote = `\`${path}\` line ${number}:\n> ${line.text.trim()}`;
      composer.setText(current.trim() ? `${current.trimEnd()}\n\n${quote}\n\n` : `${quote}\n\n`);
      selectTab(conversationId, CHAT_TAB);
      requestAnimationFrame(() => document.querySelector<HTMLElement>(COMPOSER_EDITABLE)?.focus());
    },
    [aui, conversationId, path],
  );
}
