import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowDown,
  ArrowUp,
  Check,
  Copy,
  FolderOpen,
  Search,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  type FC,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { useCheckoutRoot } from "@/components/assistant-ui/markdown-text";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { request, RequestError, revealPath } from "@/ipc/client";
import type { CheckoutFile } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";
import { HIGHLIGHT_CHARS, highlight, languageOf, type Token } from "@/lib/highlight";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { useCheckoutChanges } from "@/state/board";
import type { FileTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * A file tab: one of the session checkout's files, read only, with its line numbers and
 * colours, ⌘F to find in it and ⌃G to go to a line. It reads the file again each time it
 * comes to the front.
 */

/** Matches marked at most; past that, finding says so. */
const MAX_MATCHES = 10_000;

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function dirName(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

type Read =
  | { state: "loading" }
  | { state: "read"; file: CheckoutFile }
  | { state: "gone" }
  | { state: "failed"; error: string };

function useFile(conversationId: string, path: string, again: number): Read {
  const [read, setRead] = useState<{ key: string; time: number; read: Read } | null>(null);
  const key = `${conversationId}:${path}`;
  useEffect(() => {
    let live = true;
    request({ method: "readFile", conversationId, path })
      .then(({ file }) => live && setRead({ key, time: again, read: { state: "read", file } }))
      .catch(
        (cause: unknown) =>
          live &&
          setRead({
            key,
            time: again,
            read:
              cause instanceof RequestError && cause.code === "notFound"
                ? { state: "gone" }
                : { state: "failed", error: cause instanceof Error ? cause.message : String(cause) },
          }),
      );
    return () => {
      live = false;
    };
  }, [conversationId, path, key, again]);
  // A read of another file must not show under this one's name.
  return read?.key === key ? read.read : { state: "loading" };
}

export function FileTabView({
  conversationId,
  tab,
  active,
}: {
  conversationId: string;
  tab: FileTab;
  active: boolean;
}) {
  const root = useCheckoutRoot();
  // Read again each time the tab comes to the front, and while in front as work lands or the
  // window gains focus: an agent or another app may have changed or removed the file.
  const [again, setAgain] = useState(0);
  const [wasActive, setWasActive] = useState(active);
  const changes = useCheckoutChanges(conversationId);
  const [seenChanges, setSeenChanges] = useState(changes);
  if (wasActive !== active || seenChanges !== changes) {
    setWasActive(active);
    setSeenChanges(changes);
    if (active) setAgain(again + 1);
  }
  useEffect(() => {
    if (!active) return;
    const readAgain = () => setAgain((count) => count + 1);
    window.addEventListener("focus", readAgain);
    return () => window.removeEventListener("focus", readAgain);
  }, [active]);
  const read = useFile(conversationId, tab.path, again);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const mac = useApp((s) => s.info?.platform === "macos");
  const absolute = root ? `${root.replace(/\/$/, "")}/${tab.path}` : tab.path;
  const file = read.state === "read" ? read.file : null;
  return (
    <div data-slot="file-tab" className="flex min-h-0 flex-1 flex-col">
      <header className="border-border flex h-control-lg shrink-0 items-center gap-2 border-b px-4">
        <FileTypeIcon name={tab.path} className="size-icon-md shrink-0" />
        <h2 className="min-w-0 flex-1 truncate text-sm" title={absolute}>
          <span className="font-medium">{baseName(tab.path)}</span>
          {dirName(tab.path) && <span className="text-muted-foreground"> {dirName(tab.path)}</span>}
        </h2>
        {file && <span className="text-muted-foreground shrink-0 text-xs">{formatBytes(file.size)}</span>}
        <TooltipIconButton
          tooltip={isCopied ? "Copied" : "Copy path"}
          size="icon-sm"
          onClick={() => copyToClipboard(tab.path)}
        >
          {isCopied ? <Check /> : <Copy />}
        </TooltipIconButton>
        <TooltipIconButton
          tooltip={mac ? "Reveal in Finder" : "Open in File Manager"}
          size="icon-sm"
          disabled={read.state === "gone"}
          onClick={() =>
            revealPath(absolute).catch((cause: unknown) =>
              toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" }),
            )
          }
        >
          <FolderOpen />
        </TooltipIconButton>
      </header>
      {read.state === "gone" ? (
        <p className="text-muted-foreground m-auto p-4 text-sm">This file was deleted or moved</p>
      ) : read.state === "failed" ? (
        <p role="alert" className="text-destructive p-4 text-sm">
          {read.error}
        </p>
      ) : !file ? null : file.text === null ? (
        <p className="text-muted-foreground p-4 text-sm">Binary file not shown</p>
      ) : (
        <>
          {file.truncated && (
            <p className="text-muted-foreground border-border shrink-0 border-b px-4 py-1.5 text-xs">
              Showing the first {formatBytes(file.text.length)} of {formatBytes(file.size)}
            </p>
          )}
          <CodeLines
            text={file.text}
            language={languageOf(tab.path)}
            line={tab.line}
            reveal={tab.reveal}
            active={active}
          />
        </>
      )}
    </div>
  );
}

type Match = { line: number; start: number; end: number };

/** A line's text in coloured pieces, with the parts in `ranges` marked (the current one more). */
function marked(
  pieces: readonly { content: string; color?: string | undefined; italic?: boolean | undefined }[],
  ranges: readonly { start: number; end: number; current: boolean }[],
): ReactNode[] {
  const out: ReactNode[] = [];
  let at = 0;
  for (const [index, piece] of pieces.entries()) {
    const start = at;
    const end = at + piece.content.length;
    at = end;
    const style = piece.color ? { color: piece.color } : undefined;
    const className = piece.italic ? "italic" : undefined;
    let cut = start;
    const parts: ReactNode[] = [];
    for (const range of ranges) {
      if (range.end <= cut || range.start >= end) continue;
      const from = Math.max(range.start, cut);
      const to = Math.min(range.end, end);
      if (from > cut) parts.push(piece.content.slice(cut - start, from - start));
      parts.push(
        <mark
          key={`${index}:${from}`}
          data-current={range.current || undefined}
          className="bg-warning/30 data-current:bg-warning/70 rounded-xs text-inherit"
        >
          {piece.content.slice(from - start, to - start)}
        </mark>,
      );
      cut = to;
    }
    if (cut < end) parts.push(piece.content.slice(cut - start));
    out.push(
      // Pieces of a line never reorder.
      // oxlint-disable-next-line react/no-array-index-key
      <span key={index} className={className} style={style}>
        {parts}
      </span>,
    );
  }
  return out;
}

const CodeLines: FC<{
  text: string;
  language: string;
  line: number | null;
  reveal: number;
  active: boolean;
}> = ({ text, language, line, reveal, active }) => {
  const lines = useMemo(() => text.replace(/\n$/, "").split("\n"), [text]);
  const [tokens, setTokens] = useState<{ text: string; tokens: Token[][] | null } | null>(null);
  const density = useApp((s) => s.settings.density);
  const mac = useApp((s) => s.info?.platform === "macos");
  const scrollRef = useRef<HTMLDivElement>(null);
  // The line gone to with ⌃G, until a link asks for another.
  const asked = `${line}:${reveal}`;
  const [goneTo, setGoneTo] = useState<{ asked: string; line: number } | null>(null);
  const target = goneTo?.asked === asked ? goneTo.line : line;
  const [finding, setFinding] = useState(false);
  const [query, setQuery] = useState("");
  const [current, setCurrent] = useState(0);
  const [going, setGoing] = useState(false);
  const findInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (text.length > HIGHLIGHT_CHARS) return;
    let live = true;
    highlight(text.replace(/\n$/, ""), language)
      .then((result) => live && setTokens({ text, tokens: result }))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [text, language]);
  const lineTokens = tokens?.text === text ? tokens.tokens : null;

  const matches = useMemo<Match[]>(() => {
    const want = query.toLowerCase();
    if (!finding || !want) return [];
    const found: Match[] = [];
    for (const [index, content] of lines.entries()) {
      const lower = content.toLowerCase();
      let from = lower.indexOf(want);
      while (from >= 0 && found.length < MAX_MATCHES) {
        found.push({ line: index + 1, start: from, end: from + want.length });
        from = lower.indexOf(want, from + want.length);
      }
      if (found.length >= MAX_MATCHES) break;
    }
    return found;
  }, [lines, query, finding]);
  const byLine = useMemo(() => {
    const map = new Map<number, { start: number; end: number; current: boolean }[]>();
    for (const [index, match] of matches.entries()) {
      const list = map.get(match.line) ?? [];
      list.push({ start: match.start, end: match.end, current: index === current });
      map.set(match.line, list);
    }
    return map;
  }, [matches, current]);

  // The app does not use React Compiler, so the virtualizer's unmemoizable API is fine here.
  // oxlint-disable-next-line react/incompatible-library
  const virtualizer = useVirtualizer({
    count: lines.length,
    getScrollElement: () => scrollRef.current,
    // A line is `leading-5`: five steps of the spacing scale.
    estimateSize: () => tokenPx("--spacing") * 5,
    overscan: 20,
  });
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [density, virtualizer]);
  // A link (again) at a line marks it and brings it into view.
  useLayoutEffect(() => {
    if (line !== null && line > 0) virtualizer.scrollToIndex(line - 1, { align: "center" });
  }, [line, reveal, virtualizer]);
  const match = matches[current];
  useLayoutEffect(() => {
    if (match) virtualizer.scrollToIndex(match.line - 1, { align: "center" });
  }, [match, virtualizer]);

  // ⌘F finds and ⌃G goes to a line (Ctrl+F and Ctrl+G off macOS), while the tab is in front.
  useEffect(() => {
    if (!active) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.altKey || event.shiftKey) return;
      if (
        event.target instanceof Element &&
        event.target.closest('[data-slot="terminal-pane"], [data-pane="browser"], [data-slot="side-panel-clip"]')
      )
        return;
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      const control = event.ctrlKey && !event.metaKey;
      if (command && event.code === "KeyF") {
        event.preventDefault();
        setGoing(false);
        setFinding(true);
        requestAnimationFrame(() => findInput.current?.select());
      } else if (control && event.code === "KeyG") {
        event.preventDefault();
        setFinding(false);
        setGoing(true);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, mac]);

  const step = (by: 1 | -1) => {
    if (!matches.length) return;
    setCurrent((at) => (at + by + matches.length) % matches.length);
  };
  const goTo = (value: string) => {
    const number = Number.parseInt(value, 10);
    if (!Number.isFinite(number)) return;
    const clamped = Math.min(lines.length, Math.max(1, number));
    setGoneTo({ asked, line: clamped });
    virtualizer.scrollToIndex(clamped - 1, { align: "center" });
    setGoing(false);
    scrollRef.current?.focus();
  };

  const gutter = `${String(lines.length).length}ch`;
  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      {(finding || going) && (
        <div
          data-slot="file-find"
          className="bg-popover text-popover-foreground border-border rounded-surface shadow-menu absolute end-4 top-2 z-10 flex h-control-lg items-center gap-1 border ps-2 pe-1"
        >
          {finding ? (
            <>
              <Search className="text-muted-foreground size-icon-sm shrink-0" />
              <input
                ref={findInput}
                // ⌘F opens it to type in at once.
                // oxlint-disable-next-line jsx-a11y/no-autofocus
                autoFocus
                value={query}
                placeholder="Find"
                aria-label="Find in file"
                onChange={(event) => {
                  setQuery(event.target.value);
                  setCurrent(0);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    step(event.shiftKey ? -1 : 1);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    setFinding(false);
                    scrollRef.current?.focus();
                  }
                }}
                className="placeholder:text-muted-foreground w-48 min-w-0 bg-transparent text-sm outline-none"
              />
              <span className="text-muted-foreground shrink-0 px-1 text-xs tabular-nums">
                {!query
                  ? ""
                  : matches.length === 0
                    ? "No results"
                    : `${current + 1} of ${matches.length >= MAX_MATCHES ? `${MAX_MATCHES}+` : matches.length}`}
              </span>
              <TooltipIconButton
                tooltip="Previous match"
                shortcut="⇧↵"
                size="icon-xs"
                disabled={!matches.length}
                onClick={() => step(-1)}
              >
                <ArrowUp />
              </TooltipIconButton>
              <TooltipIconButton
                tooltip="Next match"
                shortcut="↵"
                size="icon-xs"
                disabled={!matches.length}
                onClick={() => step(1)}
              >
                <ArrowDown />
              </TooltipIconButton>
            </>
          ) : (
            <input
              // ⌃G opens it to type in at once.
              // oxlint-disable-next-line jsx-a11y/no-autofocus
              autoFocus
              inputMode="numeric"
              placeholder={`Go to line (1–${lines.length})`}
              aria-label="Go to line"
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  goTo(event.currentTarget.value);
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  setGoing(false);
                  scrollRef.current?.focus();
                }
              }}
              className="placeholder:text-muted-foreground w-48 min-w-0 bg-transparent text-sm outline-none"
            />
          )}
          <TooltipIconButton
            tooltip="Close"
            shortcut="Esc"
            size="icon-xs"
            onClick={() => {
              setFinding(false);
              setGoing(false);
            }}
          >
            <X />
          </TooltipIconButton>
        </div>
      )}
      <div
        ref={scrollRef}
        data-selectable
        tabIndex={-1}
        className="min-h-0 flex-1 overflow-auto py-2 outline-none"
      >
        <div
          className="relative min-w-full font-mono text-xs"
          style={{ height: `${virtualizer.getTotalSize()}px` }}
        >
          {virtualizer.getVirtualItems().map((item) => {
            const number = item.index + 1;
            const pieces = lineTokens?.[item.index] ?? [{ content: lines[item.index] ?? "" }];
            const ranges = byLine.get(number);
            return (
              <div
                key={item.key}
                data-line={number}
                data-target={number === target || undefined}
                className="data-target:bg-warning/15 absolute start-0 flex h-5 w-max min-w-full items-center leading-5 whitespace-pre"
                style={{ transform: `translateY(${item.start}px)` }}
              >
                <span
                  className="text-muted-foreground shrink-0 ps-4 pe-4 text-end select-none"
                  style={{ width: `calc(${gutter} + var(--spacing) * 8)` }}
                >
                  {number}
                </span>
                <span className={cn("pe-4")}>{marked(pieces, ranges ?? [])}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
};
