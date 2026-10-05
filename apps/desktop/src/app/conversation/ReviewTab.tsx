import { useAui } from "@assistant-ui/react";
import {
  Check,
  ChevronDown,
  ChevronDownUp,
  ChevronRight,
  ChevronUpDown,
  Compare,
  Copy,
  DotsHorizontal,
  ExternalLink,
  FolderOpen,
  Folders,
  JumpToCaption,
  Plus,
  Reload,
  Search,
  SidebarRight,
  Text,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  createContext,
  type FC,
  Fragment,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { useCheckoutRoot } from "@/components/assistant-ui/markdown-text";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import { openFolder, request, revealPath } from "@/ipc/client";
import type { ReviewDiff, ReviewFile, ReviewScope } from "@/ipc/generated";
import { formatSentAt } from "@/lib/format";
import { HIGHLIGHT_CHARS, highlight, languageOf, type Token } from "@/lib/highlight";
import { changedSpan, type DiffLine, type DiffRow, filePatch, type PatchFile, parsePatch } from "@/lib/patch";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import {
  LATEST_TURN,
  type ReviewOptions,
  setReviewOption,
  setReviewScope,
  useReview,
} from "@/state/review";
import { CHAT_TAB, openFileTab, openReviewTab, type ReviewTarget, selectTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";

/**
 * A session's Review tab: a scope ("Last Turn", "Uncommitted", …) and its total, every changed
 * file's diff stacked under a sticky header, and the file list beside them; or, opened from
 * the Source panel, one file's staged or unstaged diff. It only reads; a line's "+" quotes it
 * into the composer to comment on.
 */

function scopeLabel(scope: ReviewScope, review: ReviewDiff | null): string {
  switch (scope.type) {
    case "lastTurn":
      return "Last Turn";
    case "uncommitted":
      return "Uncommitted";
    case "unstaged":
      return "Unstaged";
    case "staged":
      return "Staged";
    case "branch":
      return "Branch";
    case "commit":
      return (
        review?.commits.find((commit) => commit.commit === scope.commit)?.subject ??
        scope.commit.slice(0, 7)
      );
  }
}

function sameScope(a: ReviewScope, b: ReviewScope): boolean {
  if (a.type !== b.type) return false;
  if (a.type === "commit" && b.type === "commit") return a.commit === b.commit;
  return true;
}

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
 * The review of `scope`, refetched when the options, the board, the Source panel's reading of
 * the checkout or `tick` change it. While a refetch loads, the last review stays only if it
 * shows the same scope and options: another scope's patch must not sit under this one's label.
 */
function useReviewDiff(conversationId: string, scope: ReviewScope, options: ReviewOptions, tick: number) {
  const changeKey = useChangeKey(conversationId);
  const checkout = useReview((s) => s.checkouts[conversationId] ?? "");
  const view = JSON.stringify([conversationId, scope, options.wholeFiles, options.ignoreWhitespace]);
  const key = JSON.stringify([view, tick, changeKey, checkout]);
  const [state, setState] = useState<{
    key: string | null;
    view: string | null;
    review: ReviewDiff | null;
    error: string | null;
  }>({ key: null, view: null, review: null, error: null });
  useEffect(() => {
    let live = true;
    const [id, wanted, wholeFiles, ignoreWhitespace] = JSON.parse(view) as [
      string,
      ReviewScope,
      boolean,
      boolean,
    ];
    request({
      method: "getReviewDiff",
      conversationId: id,
      scope: wanted,
      wholeFiles,
      ignoreWhitespace,
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
  target = { type: "all" },
  active = true,
}: {
  conversationId: string;
  target?: ReviewTarget;
  /** In front: it shows changes made meanwhile when it comes to the front again. */
  active?: boolean;
}) {
  const picked = useReview((s) => s.scopes[conversationId] ?? LATEST_TURN);
  const single = target.type === "file" ? target : null;
  const scope = useMemo<ReviewScope>(
    () => (single ? { type: single.staged ? "staged" : "unstaged" } : picked),
    [single, picked],
  );
  const options = useReview((s) => s.options);
  const [tick, setTick] = useState(0);
  const [wasActive, setWasActive] = useState(active);
  if (wasActive !== active) {
    setWasActive(active);
    if (active) setTick(tick + 1);
  }
  const { review: fullReview, error, loading } = useReviewDiff(conversationId, scope, options, tick);
  // One file's diff: only its part of the scope's.
  const review = useMemo(
    () =>
      fullReview && single
        ? (() => {
            const files = fullReview.files.filter((file) => file.path === single.path);
            return {
              ...fullReview,
              files,
              patch: filePatch(fullReview.patch, single.path),
              insertions: files.reduce((sum, file) => sum + (file.insertions ?? 0), 0),
              deletions: files.reduce((sum, file) => sum + (file.deletions ?? 0), 0),
            };
          })()
        : fullReview,
    [fullReview, single],
  );
  const patches = useMemo(() => {
    if (!fullReview) return new Map<string, PatchFile>();
    const parsed = parsePatch(
      fullReview.patch,
      fullReview.files.map((file) => file.path),
      fullReview.fullFiles,
    );
    return new Map(fullReview.files.map((file, index) => [file.path, parsed[index]!]));
  }, [fullReview]);
  const files = useMemo(() => review?.files ?? [], [review]);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const sections = useRef(new Map<string, HTMLElement>());
  const jump = (path: string) => {
    setCollapsed((current) => {
      const next = new Set(current);
      next.delete(path);
      return next;
    });
    requestAnimationFrame(() => sections.current.get(path)?.scrollIntoView({ block: "start" }));
  };
  const allCollapsed = files.length > 0 && files.every((file) => collapsed.has(file.path));
  const whole = review?.fullFiles ?? false;
  // Only these compare against the working tree, whose files the checkout holds as shown.
  const checkout = scope.type === "uncommitted" || scope.type === "unstaged";
  const reviewContext = useMemo(
    () => ({ conversationId, whole, checkout }),
    [conversationId, whole, checkout],
  );

  return (
    <div data-slot="review-tab" className="flex min-h-0 flex-1 flex-col">
      <div className="border-border flex h-control-lg shrink-0 items-center gap-1 border-b px-2">
        {single ? (
          <>
            <span className="text-muted-foreground px-1.5 text-sm">
              {single.staged ? "Staged changes" : "Changes"}
            </span>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setReviewScope(conversationId, { type: "uncommitted" });
                openReviewTab(conversationId, { type: "all" });
              }}
            >
              Review all
            </Button>
          </>
        ) : (
          <ScopeMenu conversationId={conversationId} scope={scope} review={review} />
        )}
        {review && review.files.length > 0 && (
          <Counts insertions={review.insertions} deletions={review.deletions} className="text-sm" />
        )}
        <div className="flex-1" />
        <OptionsMenu options={options} review={review} />
        <JumpToFile files={review?.files ?? []} onJump={jump} />
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
        <ToggleButton
          tooltip={options.split ? "Switch to unified diff" : "Switch to split diff"}
          on={options.split}
          onClick={() => setReviewOption("split", !options.split)}
        >
          <Compare />
        </ToggleButton>
        <ToggleButton
          tooltip={options.hideFiles ? "Show files" : "Hide files"}
          on={!options.hideFiles}
          onClick={() => setReviewOption("hideFiles", !options.hideFiles)}
        >
          <SidebarRight />
        </ToggleButton>
      </div>
      <div className="flex min-h-0 flex-1">
        <ReviewContext.Provider value={reviewContext}>
          <div data-slot="review-diffs" className="min-w-0 flex-1 overflow-auto">
            {error ? (
              <p className="text-destructive p-4 text-sm">{error}</p>
            ) : review && review.files.length === 0 ? (
              <p className="text-muted-foreground p-4 text-sm">
                {single ? `No ${single.staged ? "staged " : ""}changes in ${single.path}` : "No changes"}
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
                  sectionRef={(element) => {
                    if (element) sections.current.set(file.path, element);
                    else sections.current.delete(file.path);
                  }}
                />
              ))
            )}
          </div>
        </ReviewContext.Provider>
        {!options.hideFiles && review && review.files.length > 0 && (
          <FileList files={review.files} onJump={jump} />
        )}
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

/** The check beside the scope shown. */
function mark(on: boolean): ReactNode {
  return on ? <Check className="ms-auto size-icon-sm" /> : null;
}

const SCOPES: { scope: ReviewScope; label: string }[] = [
  { scope: LATEST_TURN, label: "Last Turn" },
  { scope: { type: "uncommitted" }, label: "Uncommitted" },
  { scope: { type: "unstaged" }, label: "Unstaged" },
  { scope: { type: "staged" }, label: "Staged" },
];

/** "Last Turn ⌄": the scope picker, with the branch's recent commits under "Committed". */
const ScopeMenu: FC<{
  conversationId: string;
  scope: ReviewScope;
  review: ReviewDiff | null;
}> = ({ conversationId, scope, review }) => {
  const now = useNow(60_000);
  const pick = (next: ReviewScope) => setReviewScope(conversationId, next);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className="max-w-1/2 font-normal">
          <span className="truncate">{scopeLabel(scope, review)}</span>
          <ChevronDown className="text-muted-foreground size-icon-xs" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-48">
        {SCOPES.slice(0, 1).map((entry) => (
          <DropdownMenuItem key={entry.label} onSelect={() => pick(entry.scope)}>
            {entry.label}
            {mark(sameScope(scope, entry.scope))}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        {SCOPES.slice(1).map((entry) => (
          <DropdownMenuItem key={entry.label} onSelect={() => pick(entry.scope)}>
            {entry.label}
            {mark(sameScope(scope, entry.scope))}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSub>
          <DropdownMenuSubTrigger disabled={!review || review.commits.length === 0}>
            Committed
            {mark(scope.type === "commit")}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="max-h-80 max-w-md overflow-y-auto">
            {review?.commits.map((commit) => (
              <DropdownMenuItem
                key={commit.commit}
                onSelect={() => pick({ type: "commit", commit: commit.commit })}
              >
                <span className="min-w-0 flex-1 truncate">{commit.subject}</span>
                <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                  {formatSentAt(commit.atMs, now)}
                </span>
                {mark(scope.type === "commit" && scope.commit === commit.commit)}
              </DropdownMenuItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => pick({ type: "branch" })}>
          Branch
          {mark(scope.type === "branch")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

/** "Review options" ⋯: how diffs load and draw, and the patch as a `git apply` command. */
const OptionsMenu: FC<{ options: ReviewOptions; review: ReviewDiff | null }> = ({
  options,
  review,
}) => (
  <DropdownMenu>
    <DropdownMenuTrigger asChild>
      <TooltipIconButton tooltip="Review options" size="icon-sm">
        <DotsHorizontal />
      </TooltipIconButton>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" className="min-w-56">
      <DropdownMenuCheckboxItem
        checked={!options.wholeFiles}
        onCheckedChange={(checked) => setReviewOption("wholeFiles", !checked)}
      >
        Don't load full files
      </DropdownMenuCheckboxItem>
      <DropdownMenuCheckboxItem
        checked={options.wordDiffs}
        onCheckedChange={(checked) => setReviewOption("wordDiffs", checked)}
      >
        Enable word diffs
      </DropdownMenuCheckboxItem>
      <DropdownMenuCheckboxItem
        checked={options.ignoreWhitespace}
        onCheckedChange={(checked) => setReviewOption("ignoreWhitespace", checked)}
      >
        Hide white space
      </DropdownMenuCheckboxItem>
      <DropdownMenuCheckboxItem
        checked={options.richPreview}
        onCheckedChange={(checked) => setReviewOption("richPreview", checked)}
      >
        Enable rich preview
      </DropdownMenuCheckboxItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem
        disabled={!review || review.patch === ""}
        onSelect={() => {
          if (!review) return;
          const patch = review.patch.endsWith("\n") ? review.patch : `${review.patch}\n`;
          navigator.clipboard
            .writeText(`git apply <<'BRIGADIER_PATCH'\n${patch}BRIGADIER_PATCH\n`)
            .then(
              () => toast("Copied git apply command"),
              () => toast("Failed to copy git apply command", { tone: "error" }),
            );
        }}
      >
        Copy git apply command
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
);

/** "Jump to file": the changed files, filtered as you type. */
const JumpToFile: FC<{ files: ReviewFile[]; onJump: (path: string) => void }> = ({
  files,
  onJump,
}) => {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const shown = files.filter((file) => file.path.toLowerCase().includes(query.toLowerCase()));
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setQuery("");
      }}
    >
      <PopoverTrigger asChild>
        <TooltipIconButton tooltip="Jump to file" size="icon-sm" disabled={files.length === 0}>
          <JumpToCaption />
        </TooltipIconButton>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-72 p-1">
        <input
          autoFocus
          value={query}
          placeholder="Jump to file…"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            const [first] = shown;
            if (event.key === "Enter" && first) {
              onJump(first.path);
              setOpen(false);
            }
          }}
          className="placeholder:text-muted-foreground h-control-md w-full bg-transparent px-2 text-sm outline-none"
        />
        <ul className="max-h-72 overflow-y-auto">
          {shown.map((file) => (
            <li key={file.path}>
              <button
                type="button"
                onClick={() => {
                  onJump(file.path);
                  setOpen(false);
                }}
                className="hover:bg-muted rounded-control flex h-control-md w-full items-center gap-2 px-2 text-start text-sm"
              >
                <FileTypeIcon name={file.path} className="size-icon-sm shrink-0" />
                <span className="min-w-0 flex-1 truncate">{file.path}</span>
              </button>
            </li>
          ))}
        </ul>
      </PopoverContent>
    </Popover>
  );
};

/** The badge for how a file changed: a square for modified, a letter for the rest. */
const StatusBadge: FC<{ file: ReviewFile }> = ({ file }) => {
  switch (file.status) {
    case "modified":
    case "typeChanged":
      return (
        <span
          aria-label="Modified"
          className="border-warning rounded-xs flex size-icon-xs shrink-0 items-center justify-center border"
        >
          <span className="bg-warning size-1 rounded-full" />
        </span>
      );
    case "added":
      return (
        <span
          aria-label="Added"
          className="border-success text-success rounded-xs flex size-icon-xs shrink-0 items-center justify-center border"
        >
          <Plus className="size-2.5" />
        </span>
      );
    case "untracked":
      return (
        <span aria-label="Untracked" className="text-success w-icon-xs shrink-0 text-center text-xs font-medium">
          U
        </span>
      );
    case "deleted":
      return (
        <span aria-label="Deleted" className="text-destructive w-icon-xs shrink-0 text-center text-xs font-medium">
          D
        </span>
      );
    case "renamed":
      return (
        <span aria-label="Renamed" className="text-warning w-icon-xs shrink-0 text-center text-xs font-medium">
          R
        </span>
      );
  }
};

function baseName(path: string): string {
  return path.split("/").pop() ?? path;
}

/** The file list beside the diffs: "Filter files…" and each file with its badge. */
const FileList: FC<{ files: ReviewFile[]; onJump: (path: string) => void }> = ({
  files,
  onJump,
}) => {
  const [filter, setFilter] = useState("");
  const shown = files.filter((file) => file.path.toLowerCase().includes(filter.toLowerCase()));
  return (
    <aside
      aria-label="Changed files"
      className="border-border flex w-1/3 max-w-64 min-w-40 shrink-0 flex-col gap-1 overflow-y-auto border-s p-2"
    >
      <label className="border-border rounded-control flex h-control-md shrink-0 items-center gap-1.5 border px-2">
        <Search className="text-muted-foreground size-icon-sm shrink-0" />
        <input
          value={filter}
          placeholder="Filter files…"
          onChange={(event) => setFilter(event.target.value)}
          className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none"
        />
      </label>
      <ul>
        {shown.map((file) => (
          <li key={file.path}>
            <button
              type="button"
              title={file.path}
              onClick={() => onJump(file.path)}
              className="hover:bg-muted rounded-control text-muted-foreground hover:text-foreground flex h-control-md w-full items-center gap-2 px-2 text-start text-sm"
            >
              <FileTypeIcon name={file.path} className="size-icon-sm shrink-0" />
              <span className="min-w-0 flex-1 truncate">{baseName(file.path)}</span>
              <StatusBadge file={file} />
            </button>
          </li>
        ))}
      </ul>
    </aside>
  );
};

/** One file's diff under its sticky header: icon, path, counts, and on hover copy and fold. */
const FileSection: FC<{
  file: ReviewFile;
  patch: PatchFile | undefined;
  options: ReviewOptions;
  collapsed: boolean;
  onToggle: () => void;
  sectionRef: (element: HTMLElement | null) => void;
}> = ({ file, patch, options, collapsed, onToggle, sectionRef }) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <section ref={sectionRef} data-slot="review-file" data-path={file.path} className="scroll-mt-0">
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
      {!collapsed &&
        (options.richPreview && MARKDOWN.has(languageOf(file.path)) && file.status !== "deleted" ? (
          <RichPreview file={file} patch={patch} />
        ) : (
          <FileBody file={file} patch={patch} options={options} />
        ))}
    </section>
  );
};

/** File types "Enable rich preview" shows rendered instead of as a diff. */
const MARKDOWN = new Set(["md", "markdown", "mdx"]);

/** The file's new text, when the patch holds all of it (whole files loaded). */
function newText(patch: PatchFile | undefined, whole: boolean): string | null {
  if (!patch || !whole) return null;
  const lines: string[] = [];
  for (const row of patch.rows) {
    if (row.kind === "gap") {
      if (!row.lines) return null;
      lines.push(...row.lines.map((line) => line.text));
    } else if (row.kind !== "del") lines.push(row.text);
  }
  return lines.join("\n");
}

/**
 * "Enable rich preview": a Markdown file rendered as it reads after the change. The text
 * comes from the diff when it holds the whole file, else from the checkout when the scope's
 * new side is the working tree; a staged or committed version needs the full files loaded.
 */
const RichPreview: FC<{ file: ReviewFile; patch: PatchFile | undefined }> = ({ file, patch }) => {
  const { conversationId, whole, checkout } = useContext(ReviewContext);
  const fromPatch = newText(patch, whole);
  const [read, setRead] = useState<{ text: string | null; error: string | null }>({
    text: null,
    error: null,
  });
  useEffect(() => {
    if (fromPatch !== null || !checkout) return;
    let live = true;
    request({ method: "readFile", conversationId, path: file.path })
      .then(({ file: found }) => live && setRead({ text: found.text ?? "", error: null }))
      .catch((cause: unknown) => {
        if (live) setRead({ text: null, error: cause instanceof Error ? cause.message : String(cause) });
      });
    return () => {
      live = false;
    };
  }, [conversationId, file.path, fromPatch, checkout]);
  if (fromPatch === null && !checkout) {
    return (
      <p className="text-muted-foreground px-3 py-2 text-sm">
        The full file isn't loaded for this version, so it can't be previewed.
      </p>
    );
  }
  const text = fromPatch ?? read.text;
  if (read.error) return <p className="text-destructive px-3 py-2 text-sm">{read.error}</p>;
  if (text === null) return null;
  return (
    <div data-slot="review-preview" className="px-4 py-3 text-sm">
      <MarkdownBlock text={text} />
    </div>
  );
};

/**
 * What a file's diff needs from its tab: whose checkout, whether whole files loaded, and
 * whether the scope's new side is the checkout's working tree.
 */
const ReviewContext = createContext<{ conversationId: string; whole: boolean; checkout: boolean }>({
  conversationId: "",
  whole: false,
  checkout: false,
});

function failed(cause: unknown) {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

/** A changed file's "Open in" menu: a file tab, its default app, or the file manager. */
const OpenInMenu: FC<{ file: ReviewFile }> = ({ file }) => {
  const { conversationId } = useContext(ReviewContext);
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

const FileBody: FC<{ file: ReviewFile; patch: PatchFile | undefined; options: ReviewOptions }> = ({
  file,
  patch,
  options,
}) => {
  const [opened, setOpened] = useState<ReadonlySet<number>>(new Set());
  const [commenting, setCommenting] = useState<string | null>(null);
  const tokens = useSideTokens(file.path, patch);
  if (!patch || file.binary || patch.binary) {
    return (
      <p className="text-muted-foreground px-3 py-2 text-sm">
        {file.binary || patch?.binary ? "Binary file not shown" : "No content changes"}
      </p>
    );
  }
  if (patch.rows.length === 0) {
    return <p className="text-muted-foreground px-3 py-2 text-sm">No content changes</p>;
  }
  // Opened stretches of unchanged lines show their lines in place.
  const rows: DiffRow[] = patch.rows.flatMap((row) =>
    row.kind === "gap" && row.lines && opened.has(row.id) ? row.lines : [row],
  );
  const open = (id: number) => setOpened((current) => new Set(current).add(id));
  const pairs = options.wordDiffs ? pairChanges(rows) : new Map<DiffLine, DiffLine>();
  return (
    <TokensContext.Provider value={tokens}>
      <div
        className={cn(
          "font-mono text-xs leading-5",
          // Split sides share the width equally, so their lines wrap.
          options.wrap || options.split
            ? "whitespace-pre-wrap break-all"
            : "w-max min-w-full whitespace-pre",
        )}
      >
        {options.split ? (
          <SplitRows
            rows={rows}
            pairs={pairs}
            onOpen={open}
          />
        ) : (
          rows.map((row, index) =>
            row.kind === "gap" ? (
              <GapRow key={`gap:${row.id}`} row={row} onOpen={open} />
            ) : (
              <Fragment key={`${row.kind}:${row.old}:${row.new}:${index}`}>
                <LineRow
                  line={row}
                  partner={pairs.get(row) ?? null}
                  onComment={() => setCommenting(lineKey(row))}
                />
                {commenting === lineKey(row) && (
                  <CommentBox
                    path={file.path}
                    line={row}
                    onClose={() => setCommenting(null)}
                  />
                )}
              </Fragment>
            ),
          )
        )}
      </div>
    </TokensContext.Provider>
  );
};

/** Each side's colours by line number: the old file's for deleted lines, the new one's else. */
type SideTokens = { old: Map<number, Token[]>; new: Map<number, Token[]> };

const TokensContext = createContext<SideTokens | null>(null);

/** Colours both sides of a file's diff, from the lines the patch holds of each. */
function useSideTokens(path: string, patch: PatchFile | undefined): SideTokens | null {
  // Kept with the patch they colour, so a new patch shows plain until its colours come.
  const [found, setFound] = useState<{ patch: PatchFile; tokens: SideTokens } | null>(null);
  useEffect(() => {
    if (!patch || patch.binary) return;
    const language = languageOf(path);
    const lines = patch.rows.flatMap((row) => (row.kind === "gap" ? (row.lines ?? []) : [row]));
    const olds = lines.filter((line) => line.kind !== "add");
    const news = lines.filter((line) => line.kind !== "del");
    const beforeText = olds.map((line) => line.text).join("\n");
    const afterText = news.map((line) => line.text).join("\n");
    if (beforeText.length + afterText.length > HIGHLIGHT_CHARS) return;
    let live = true;
    Promise.all([highlight(beforeText, language), highlight(afterText, language)])
      .then(([before, after]) => {
        if (!live || !before || !after) return;
        setFound({
          patch,
          tokens: { old: byNumber(olds, before, "old"), new: byNumber(news, after, "new") },
        });
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [path, patch]);
  return found && found.patch === patch ? found.tokens : null;
}

function byNumber(side: DiffLine[], tokens: Token[][], key: "old" | "new"): Map<number, Token[]> {
  return new Map(side.map((line, index) => [line[key] ?? 0, tokens[index] ?? []]));
}

/** `text` in its colours, the part in `span` marked (word diffs). */
function Coloured({
  tokens,
  span,
  kind,
}: {
  tokens: Token[];
  span: [number, number] | null;
  kind: DiffLine["kind"];
}) {
  const parts: ReactNode[] = [];
  let offset = 0;
  tokens.forEach((token, index) => {
    const start = offset;
    const end = offset + token.content.length;
    offset = end;
    const style = token.color ? { color: token.color } : undefined;
    const italic = cn(token.italic && "italic");
    // A token is cut where the marked span starts and ends.
    const cuts = [start, ...(span ?? []).filter((at) => at > start && at < end), end];
    for (let piece = 0; piece < cuts.length - 1; piece += 1) {
      const from = cuts[piece] as number;
      const to = cuts[piece + 1] as number;
      const text = token.content.slice(from - start, to - start);
      const marked = span !== null && from >= span[0] && to <= span[1];
      parts.push(
        marked ? (
          <mark
            key={`${index}:${piece}`}
            className={cn(
              "rounded-xs",
              italic,
              kind === "add" ? "bg-success/30" : "bg-destructive/30",
            )}
            style={style}
          >
            {text}
          </mark>
        ) : (
          <span key={`${index}:${piece}`} className={italic} style={style}>
            {text}
          </span>
        ),
      );
    }
  });
  return <>{parts}</>;
}

function lineKey(line: DiffLine): string {
  return `${line.kind}:${line.old}:${line.new}`;
}

/** Each deleted line with the added line that took its place, for word diffs. */
function pairChanges(rows: DiffRow[]): Map<DiffLine, DiffLine> {
  const pairs = new Map<DiffLine, DiffLine>();
  let index = 0;
  while (index < rows.length) {
    const deleted: DiffLine[] = [];
    while (rows[index]?.kind === "del") deleted.push(rows[index++] as DiffLine);
    const added: DiffLine[] = [];
    while (rows[index]?.kind === "add") added.push(rows[index++] as DiffLine);
    for (let pair = 0; pair < Math.min(deleted.length, added.length); pair += 1) {
      const before = deleted[pair] as DiffLine;
      const after = added[pair] as DiffLine;
      pairs.set(before, after);
      pairs.set(after, before);
    }
    if (deleted.length === 0 && added.length === 0) index += 1;
  }
  return pairs;
}

const TINT: Record<DiffLine["kind"], string> = {
  context: "",
  add: "bg-success/10 border-success",
  del: "bg-destructive/10 border-destructive",
};

/** A line's text, with the part that differs from its partner marked (word diffs). */
function LineText({ line, partner }: { line: DiffLine; partner: DiffLine | null }) {
  const sides = useContext(TokensContext);
  const tokens = sides?.[line.kind === "del" ? "old" : "new"].get(
    (line.kind === "del" ? line.old : line.new) ?? 0,
  );
  const span: [number, number] | null =
    !partner || line.kind === "context"
      ? null
      : line.kind === "del"
        ? changedSpan(line.text, partner.text).before
        : changedSpan(partner.text, line.text).after;
  if (tokens && tokens.length > 0) {
    return (
      <Coloured tokens={tokens} span={span && span[0] < span[1] ? span : null} kind={line.kind} />
    );
  }
  if (!span) return <>{line.text || " "}</>;
  const [start, end] = span;
  if (start >= end) return <>{line.text || " "}</>;
  return (
    <>
      {line.text.slice(0, start)}
      <mark
        className={cn(
          "text-inherit rounded-xs",
          line.kind === "add" ? "bg-success/30" : "bg-destructive/30",
        )}
      >
        {line.text.slice(start, end)}
      </mark>
      {line.text.slice(end)}
    </>
  );
}

/** One line of a unified diff: its number, a "+" to comment on hover, and its text. */
const LineRow: FC<{ line: DiffLine; partner: DiffLine | null; onComment: () => void }> = ({
  line,
  partner,
  onComment,
}) => (
  <div
    data-kind={line.kind}
    className={cn("group/line flex border-s-2 border-transparent", TINT[line.kind])}
  >
    <span className="text-muted-foreground relative w-12 shrink-0 pe-3 text-end tabular-nums select-none">
      {line.kind === "del" ? line.old : line.new}
      <button
        type="button"
        aria-label="Add comment"
        onClick={onComment}
        className="bg-link text-background rounded-xs absolute start-0.5 top-0.5 hidden size-4 items-center justify-center group-hover/line:flex"
      >
        <Plus className="size-3" />
      </button>
    </span>
    <span className="min-w-0 flex-1 pe-3">
      <LineText line={line} partner={partner} />
    </span>
  </div>
);

/** "12 unmodified lines": a stretch of unchanged lines, opened by a click when loaded. */
const GapRow: FC<{ row: Extract<DiffRow, { kind: "gap" }>; onOpen: (id: number) => void }> = ({
  row,
  onOpen,
}) => {
  const label = `${row.count} unmodified ${row.count === 1 ? "line" : "lines"}`;
  const className =
    "bg-muted/60 text-muted-foreground flex h-control-sm w-full items-center px-3 font-sans text-xs";
  return row.lines ? (
    <button type="button" onClick={() => onOpen(row.id)} className={cn(className, "hover:text-foreground")}>
      {label}
    </button>
  ) : (
    <div className={className}>{label}</div>
  );
};

/** Split diff: the old file on the left, the new on the right, changes side by side. */
const SplitRows: FC<{
  rows: DiffRow[];
  pairs: Map<DiffLine, DiffLine>;
  onOpen: (id: number) => void;
}> = ({ rows, pairs, onOpen }) => {
  const lines: ReactNode[] = [];
  let index = 0;
  while (index < rows.length) {
    const row = rows[index] as DiffRow;
    if (row.kind === "gap") {
      lines.push(<GapRow key={`gap:${row.id}`} row={row} onOpen={onOpen} />);
      index += 1;
      continue;
    }
    if (row.kind === "context") {
      lines.push(<SplitLine key={`c:${index}`} left={row} right={row} pairs={pairs} />);
      index += 1;
      continue;
    }
    const deleted: DiffLine[] = [];
    while (rows[index]?.kind === "del") deleted.push(rows[index++] as DiffLine);
    const added: DiffLine[] = [];
    while (rows[index]?.kind === "add") added.push(rows[index++] as DiffLine);
    for (let pair = 0; pair < Math.max(deleted.length, added.length); pair += 1) {
      lines.push(
        <SplitLine
          key={`p:${index}:${pair}`}
          left={deleted[pair] ?? null}
          right={added[pair] ?? null}
          pairs={pairs}
        />,
      );
    }
  }
  return <>{lines}</>;
};

const SplitSide: FC<{ line: DiffLine | null; pairs: Map<DiffLine, DiffLine>; side: "old" | "new" }> = ({
  line,
  pairs,
  side,
}) => (
  <div
    className={cn(
      "flex min-w-0 flex-1 basis-0 border-s-2 border-transparent",
      line ? TINT[line.kind] : "bg-muted/30",
    )}
  >
    <span className="text-muted-foreground w-12 shrink-0 pe-3 text-end tabular-nums select-none">
      {line ? (side === "old" ? line.old : line.new) : ""}
    </span>
    <span className="min-w-0 flex-1 pe-3">
      {line && <LineText line={line} partner={pairs.get(line) ?? null} />}
    </span>
  </div>
);

const SplitLine: FC<{
  left: DiffLine | null;
  right: DiffLine | null;
  pairs: Map<DiffLine, DiffLine>;
}> = ({ left, right, pairs }) => (
  <div className="flex">
    <SplitSide line={left} pairs={pairs} side="old" />
    <SplitSide line={right} pairs={pairs} side="new" />
  </div>
);

/**
 * The box a line's "+" opens: the comment goes into the composer with the line quoted, to
 * send to the orchestrator like any message. Chat comes to the front to show it.
 */
const CommentBox: FC<{ path: string; line: DiffLine; onClose: () => void }> = ({
  path,
  line,
  onClose,
}) => {
  const aui = useAui();
  const { conversationId } = useContext(ReviewContext);
  const [text, setText] = useState("");
  const number = line.kind === "del" ? line.old : line.new;
  const submit = () => {
    const composer = aui.composer();
    const current = composer.getState().text;
    const quote = `\`${path}\` line ${number}:\n> ${line.text.trim()}\n${text.trim()}`;
    composer.setText(current.trim() ? `${current.trimEnd()}\n\n${quote}` : quote);
    onClose();
    selectTab(conversationId, CHAT_TAB);
    requestAnimationFrame(() => document.querySelector<HTMLElement>(COMPOSER_EDITABLE)?.focus());
  };
  return (
    <div className="border-border bg-card my-1 ms-12 me-3 flex flex-col gap-2 rounded-xl border p-2 font-sans text-sm whitespace-normal">
      <textarea
        autoFocus
        rows={2}
        value={text}
        placeholder="Add a comment…"
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && text.trim()) submit();
        }}
        className="placeholder:text-muted-foreground resize-none bg-transparent outline-none"
      />
      <div className="flex justify-end gap-1">
        <Button variant="ghost" size="xs" onClick={onClose}>
          Cancel
        </Button>
        <Button size="xs" disabled={!text.trim()} onClick={submit}>
          Comment
        </Button>
      </div>
    </div>
  );
};
