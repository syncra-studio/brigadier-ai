import type { SyntaxHighlighterProps } from "@assistant-ui/react-markdown";
import {
  type CSSProperties,
  type FC,
  type ReactNode,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { holdInThread } from "@/components/conversation/thread-scroll";
import { CopyButton } from "@/components/shared/copy-button";
import { Button } from "@/components/controls/button";
import { Kbd } from "@/components/controls/kbd";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { Dialog, DialogClose, DialogContent, DialogTitle } from "@/components/overlays/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays/dropdown-menu";
import { ArrowRight, ChevronDown, ChevronUp, Code, DotsHorizontal, ExpandLg, Spinner, X } from "@/components/icons";
import { useFindSearching } from "@/lib/findSearching";
import type { Token } from "@/lib/languages";
import { cn } from "@/lib/utils";

/** How a block's language is named in its header ("Bash", "Diff", "TypeScript"). */
const NAMES: Record<string, string> = {
  sh: "Bash",
  bash: "Bash",
  shell: "Shell",
  zsh: "Zsh",
  js: "JavaScript",
  javascript: "JavaScript",
  jsx: "JSX",
  ts: "TypeScript",
  typescript: "TypeScript",
  tsx: "TSX",
  py: "Python",
  python: "Python",
  rs: "Rust",
  rust: "Rust",
  json: "JSON",
  yaml: "YAML",
  yml: "YAML",
  toml: "TOML",
  md: "Markdown",
  markdown: "Markdown",
  html: "HTML",
  css: "CSS",
  sql: "SQL",
  diff: "Diff",
  patch: "Diff",
  go: "Go",
};

function languageName(language: string): string {
  const key = language.toLowerCase();
  return NAMES[key] ?? (key ? key.charAt(0).toUpperCase() + key.slice(1) : "Text");
}

const isDiff = (language: string) => /^(diff|patch)$/i.test(language);

/** The block's lines as coloured tokens once shiki has read them; plain text until then. */
function useTokens(code: string, language: string): Token[][] | null {
  const [tokens, setTokens] = useState<{ code: string; lines: Token[][] | null } | null>(null);
  useEffect(() => {
    if (!language || isDiff(language)) return;
    let live = true;
    // While a block streams its code changes often: read it once it settles for a moment.
    const timer = window.setTimeout(() => {
      import("@/lib/highlight")
        .then(({ highlight }) => highlight(code, language))
        .then((lines) => live && setTokens({ code, lines }))
        .catch(() => live && setTokens({ code, lines: null }));
    }, 120);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [code, language]);
  return tokens?.code === code ? tokens.lines : null;
}

/** A line as it shows: its content, and a diff's colours for its row and its number (the row's tint over the card). */
type Line = { content: ReactNode; row?: string; gutter?: string };

/** A diff's line in colour: + green and − red on a tinted row, @@ blue, file headers grey italic. */
function diffLine(line: string): Line {
  if (line.startsWith("+++") || line.startsWith("---")) return { content: line, row: "text-muted-foreground italic" };
  if (line.startsWith("+")) {
    return { content: line, row: "bg-success/12 text-success", gutter: "bg-linear-to-r from-success/12 to-success/12" };
  }
  if (line.startsWith("-")) {
    return { content: line, row: "bg-destructive/12 text-destructive", gutter: "bg-linear-to-r from-destructive/12 to-destructive/12" };
  }
  if (line.startsWith("@@")) return { content: line, row: "text-link" };
  return { content: line };
}

/** The block's lines: a diff in its own colours, other code syntax-highlighted once read. */
function useLines(text: string, language: string): Line[] {
  const tokens = useTokens(text, language);
  return useMemo(() => {
    if (isDiff(language)) return text.split("\n").map(diffLine);
    if (!tokens) return text.split("\n").map((line) => ({ content: line }));
    return tokens.map((line) => ({
      content: line.map((token, at) => (
        <span
          // oxlint-disable-next-line react/no-array-index-key -- tokens have no identity
          key={at}
          style={{ color: token.color }}
          className={cn(token.italic && "italic")}
        >
          {token.content}
        </span>
      )),
    }));
  }, [text, language, tokens]);
}

/** A diff's added and removed lines (file headers aside). */
function diffCounts(text: string): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const line of text.split("\n")) {
    if (line.startsWith("+") && !line.startsWith("+++")) added += 1;
    else if (line.startsWith("-") && !line.startsWith("---")) removed += 1;
  }
  return { added, removed };
}

/**
 * The lines, each a row with its number in a gutter that stays put while long lines scroll
 * sideways (`card` 48 wide, `dialog` 56), or none. The number is smaller than the code but sits
 * on its first line.
 */
const Lines: FC<{ lines: Line[]; gutter: "card" | "dialog" | null; wrap: boolean }> = ({ lines, gutter, wrap }) => (
  <code className="block">
    {lines.map((line, index) => (
      // oxlint-disable-next-line react/no-array-index-key -- lines have no identity
      <span key={index} data-line className={cn("flex", !wrap && "w-max min-w-full", line.row)}>
        {gutter && (
          <span
            aria-hidden
            className={cn(
              "text-muted-foreground bg-code-surface sticky start-0 shrink-0 text-end not-italic select-none",
              gutter === "card" ? "w-code-gutter pe-3" : "w-code-gutter-lg pe-4",
              line.gutter,
            )}
          >
            <span className="text-xs">{index + 1}</span>
          </span>
        )}
        <span className={cn(wrap ? "min-w-0 flex-1 break-words whitespace-pre-wrap" : "whitespace-pre", gutter && "pe-3")}>
          {line.content === "" ? " " : line.content}
        </span>
      </span>
    ))}
  </code>
);

type Edges = { start: boolean; end: boolean };
const NO_EDGES: Edges = { start: false, end: false };
/** How far a side with more code fades out. */
const FADE = "var(--spacing-row-sm)";

/** Which sides of a box scrolling sideways have more past them, as it scrolls, resizes and its code streams in. */
function useMoreSideways(ref: RefObject<HTMLElement | null>, on: boolean): Edges {
  const [edges, setEdges] = useState<Edges>(NO_EDGES);
  useLayoutEffect(() => {
    const box = ref.current;
    if (!box || !on) return;
    const measure = () => {
      const start = box.scrollLeft > 1;
      const end = box.scrollLeft < box.scrollWidth - box.clientWidth - 1;
      setEdges((before) => (before.start === start && before.end === end ? before : { start, end }));
    };
    measure();
    box.addEventListener("scroll", measure, { passive: true });
    const resized = new ResizeObserver(measure);
    resized.observe(box);
    const changed = new MutationObserver(measure);
    changed.observe(box, { childList: true, subtree: true, characterData: true });
    return () => {
      box.removeEventListener("scroll", measure);
      resized.disconnect();
      changed.disconnect();
    };
  }, [ref, on]);
  return on ? edges : NO_EDGES;
}

/** The fade on the sides with more code, the start one after the gutter (`gutter`) if any. */
function sideFade(more: Edges, gutter?: string): CSSProperties | undefined {
  if (!more.start && !more.end) return undefined;
  const start = !more.start
    ? "black"
    : gutter
      ? `black ${gutter}, transparent ${gutter}, black calc(${gutter} + ${FADE})`
      : `transparent, black ${FADE}`;
  const end = more.end ? `black calc(100% - ${FADE}), transparent` : "black";
  return { maskImage: `linear-gradient(to right, ${start}, ${end})` };
}

/** A block of more lines than this starts folded; one of this many or fewer always shows whole. */
const FOLD_OVER = 14;
/** How many rows a folded block shows (visual rows: a wrapped line takes several). */
const FOLDED_ROWS = 10;
/**
 * The lines a folded block mounts: its rows and the one under them that its fade covers. The rest
 * mount on Show more, or while Find has a text, so they are found: a long output's lines held in
 * the page, unseen, weighed tens of MB each (#569).
 */
const FOLDED_LINES = FOLDED_ROWS + 1;

/**
 * Whether each block shows all its lines, kept by the block (its language, place in its text and
 * first line) for the app's run: the stored reply replacing the streamed one, or a work block
 * folding, mounts the block again with what the user chose.
 */
const folds = new Map<string, boolean>();

function useBlockFold(key: string): [boolean, (expanded: boolean) => void] {
  const [expanded, setExpanded] = useState(() => folds.get(key) ?? false);
  return [
    expanded,
    (next) => {
      folds.set(key, next);
      setExpanded(next);
    },
  ];
}

/**
 * The first line a folded block hides: the first that starts below its `FOLDED_ROWS` rows,
 * measured as the code wraps and grows.
 */
function useFirstHidden(ref: RefObject<HTMLElement | null>, on: boolean): number {
  const [first, setFirst] = useState(FOLDED_ROWS + 1);
  useLayoutEffect(() => {
    const box = ref.current;
    if (!box || !on) return;
    const measure = () => {
      const shown = FOLDED_ROWS * Number.parseFloat(getComputedStyle(box).lineHeight);
      const rows = Array.from(box.querySelectorAll<HTMLElement>("[data-line]"));
      const hidden = rows.findIndex((row) => row.offsetTop >= shown - 1);
      setFirst((hidden === -1 ? rows.length : hidden) + 1);
    };
    measure();
    // Rows move as the box resizes, and as lines wrap or arrive, which a folded box doesn't resize for.
    const resized = new ResizeObserver(measure);
    resized.observe(box);
    const changed = new MutationObserver(measure);
    changed.observe(box, { childList: true, subtree: true, characterData: true, attributes: true });
    return () => {
      resized.disconnect();
      changed.disconnect();
    };
  }, [ref, on]);
  return first;
}

/** Whether the header sticks at the top of the thread's view (`marker` at the card's top has left it). */
function useStuck(marker: RefObject<HTMLElement | null>, on: boolean): boolean {
  const [stuck, setStuck] = useState(false);
  useEffect(() => {
    const element = marker.current;
    if (!element || !on) return;
    const seen = new IntersectionObserver(
      ([entry]) => {
        if (entry) setStuck(!entry.isIntersecting && entry.boundingClientRect.top < (entry.rootBounds?.top ?? 0));
      },
      { root: element.closest("[data-slot=aui_thread-viewport]") },
    );
    seen.observe(element);
    return () => seen.disconnect();
  }, [marker, on]);
  return on && stuck;
}

const lineWord = (count: number) => `${count} ${count === 1 ? "line" : "lines"}`;

/**
 * The code block: a card with `</>`, the language and, on a long block, its size on the left;
 * Wrap, Open in a larger view and Copy on the right. The code is syntax-highlighted (a diff in its
 * own colours, its rows tinted); long lines scroll sideways, the side with more fading out.
 *
 * A block of more than `FOLD_OVER` lines starts folded to its first `FOLDED_ROWS` rows, numbered,
 * the last fading into the card, with "Show more · lines 11–60" under it. Expanded, Show less
 * joins the header and ends the block, and the header sticks to the top of the thread while the
 * block is on screen. Folding keeps the header where it is on screen; other blocks keep their own
 * fold. Copy always copies the whole block. While the block is still being written, the header
 * says so with its line count. A mermaid block draws its diagram.
 */
export const CodeBlock: FC<SyntaxHighlighterProps> = ({ code, language, node }) => {
  if (language.toLowerCase() === "mermaid") return <Diagram code={code} />;
  return <CodeCard code={code} language={language} node={node} />;
};

const CodeCard: FC<Pick<SyntaxHighlighterProps, "code" | "language" | "node">> = ({ code, language, node }) => {
  const text = code.endsWith("\n") ? code.slice(0, -1) : code;
  const lines = useLines(text, language);
  const total = lines.length;
  const foldable = total > FOLD_OVER;
  const writing = node?.properties["dataOpen"] === true;
  const [expanded, setExpanded] = useBlockFold(
    `${language}:${node?.position?.start.offset ?? ""}:${text.split("\n", 1)[0]}`,
  );
  const folded = foldable && !expanded;
  const [wrap, setWrap] = useState(false);
  const [larger, setLarger] = useState(false);
  const preRef = useRef<HTMLPreElement>(null);
  const markerRef = useRef<HTMLSpanElement>(null);
  const wrapRef = useRef<HTMLButtonElement>(null);
  const openRef = useRef<HTMLButtonElement>(null);
  const more = useMoreSideways(preRef, !wrap);
  const firstHidden = useFirstHidden(preRef, folded);
  const stuck = useStuck(markerRef, foldable && expanded);
  const searched = useFindSearching();
  const shown = folded && !searched ? lines.slice(0, FOLDED_LINES) : lines;
  const name = languageName(language);
  const diff = isDiff(language) ? diffCounts(text) : null;
  const fold = (next: boolean) => {
    // Opening or closing, the header stays where it is on screen and the rest moves.
    holdInThread(wrapRef.current);
    setExpanded(next);
  };
  return (
    <div
      data-slot="code-block"
      className="border-border bg-code-surface relative my-3 overflow-clip rounded-2xl border first:mt-0 last:mb-0"
    >
      <span ref={markerRef} aria-hidden className="pointer-events-none absolute inset-x-0 top-0 h-px" />
      <div
        className={cn(
          "bg-code-surface text-muted-foreground flex h-control-md items-center gap-1.5 ps-3 pe-1.5 text-xs",
          foldable && expanded && "sticky top-0 z-1",
          stuck && "shadow-code-sticky",
        )}
      >
        <Code aria-hidden className="size-icon-sm shrink-0" />
        <span className="flex min-w-0 flex-1 items-center gap-1.5 truncate">
          <span className="font-medium">{name}</span>
          {writing ? (
            <>
              <Spinner aria-hidden className="size-icon-xs animate-spin motion-reduce:animate-none" />
              <span>Writing… {lineWord(total)}</span>
            </>
          ) : diff && foldable ? (
            <span>
              · <span className="text-success">+{diff.added}</span>{" "}
              <span className="text-destructive">−{diff.removed}</span> · {lineWord(total)}
            </span>
          ) : (
            foldable && <span>· {lineWord(total)}</span>
          )}
        </span>
        {foldable && expanded && (
          <Button
            variant="ghost"
            size="xs"
            aria-expanded
            className="text-muted-foreground hover:bg-foreground/8 h-5 px-1.5"
            onClick={() => fold(false)}
          >
            <ChevronUp aria-hidden />
            Show less
          </Button>
        )}
        <TooltipIconButton
          ref={wrapRef}
          tooltip={wrap ? "Turn off word wrap" : "Enable word wrap"}
          size="icon-xs"
          pressed={wrap}
          className="aria-pressed:bg-toolbar-active aria-pressed:text-foreground"
          onClick={() => setWrap(!wrap)}
        >
          <ArrowRight />
        </TooltipIconButton>
        <TooltipIconButton ref={openRef} tooltip="Open in a larger view" size="icon-xs" onClick={() => setLarger(true)}>
          <ExpandLg />
        </TooltipIconButton>
        <CopyButton
          text={code}
          size="icon-xs"
          copiedTooltip={foldable ? `Copied ${lineWord(total)}` : "Copied"}
          copiedDuration={1500}
        />
      </div>
      <div className="relative">
        <pre
          ref={preRef}
          data-more={more.start && more.end ? "both" : more.start ? "start" : more.end ? "end" : undefined}
          className={cn(
            "text-code relative font-mono text-sm leading-relaxed",
            foldable ? (folded ? "max-h-code-folded" : "pb-2") : "px-3 pb-3",
            wrap ? "overflow-x-hidden" : "overflow-x-auto",
            folded ? "overflow-y-hidden" : "overflow-y-clip",
          )}
          style={sideFade(more, foldable ? "var(--spacing-code-gutter)" : undefined)}
        >
          <Lines lines={shown} gutter={foldable ? "card" : null} wrap={wrap} />
        </pre>
        {folded && (
          <div aria-hidden className="from-code-surface pointer-events-none absolute inset-x-0 bottom-0 h-11.5 bg-linear-to-t" />
        )}
      </div>
      {foldable && (
        <button
          type="button"
          aria-expanded={expanded}
          className="border-divider text-muted-foreground hover:bg-foreground/5 hover:text-foreground flex h-control-md w-full items-center gap-1.5 border-t px-3 text-xs transition-colors"
          onClick={() => fold(!expanded)}
        >
          {expanded ? <ChevronUp aria-hidden className="size-icon-sm" /> : <ChevronDown aria-hidden className="size-icon-sm" />}
          <span className="font-medium">{expanded ? "Show less" : "Show more"}</span>
          {!expanded && (
            <span>
              · lines {firstHidden}–{total}
              {writing && " so far"}
            </span>
          )}
        </button>
      )}
      <CodeBlockDialog
        open={larger}
        onOpenChange={setLarger}
        returnTo={openRef}
        code={code}
        lines={lines}
        title={name}
        size={lineWord(total)}
      />
    </div>
  );
};

/**
 * The larger view of one block, over the thread at 80% of the window: its language and size, its
 * own Wrap, Copy and Close, then every line numbered, scrolling inside. Esc, Close or a click
 * outside closes it and puts focus back on the block's Open button.
 */
const CodeBlockDialog: FC<{
  open: boolean;
  onOpenChange: (open: boolean) => void;
  returnTo: RefObject<HTMLButtonElement | null>;
  code: string;
  lines: Line[];
  title: string;
  size: string;
}> = ({ open, onOpenChange, returnTo, code, lines, title, size }) => {
  const [wrap, setWrap] = useState(false);
  const preRef = useRef<HTMLPreElement>(null);
  const more = useMoreSideways(preRef, open && !wrap);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-slot="code-block-dialog"
        showCloseButton={false}
        aria-describedby={undefined}
        className="flex h-4/5 w-4/5 max-w-none flex-col gap-0 overflow-hidden p-0"
        onOpenAutoFocus={(event) => {
          // The code takes focus, so the keys scroll it (the first button's tip would show).
          event.preventDefault();
          preRef.current?.focus();
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          returnTo.current?.focus();
        }}
      >
        <div className="border-divider flex h-12 shrink-0 items-center gap-2 border-b ps-5 pe-3">
          <Code aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
          <div className="flex min-w-0 flex-1 items-center gap-2 truncate text-sm">
            <DialogTitle className="text-foreground text-sm font-medium">{title}</DialogTitle>
            <span className="text-muted-foreground">· {size}</span>
          </div>
          <TooltipIconButton
            tooltip={wrap ? "Turn off word wrap" : "Enable word wrap"}
            size="icon-sm"
            pressed={wrap}
            className="aria-pressed:bg-toolbar-active aria-pressed:text-foreground"
            onClick={() => setWrap(!wrap)}
          >
            <ArrowRight />
          </TooltipIconButton>
          <CopyButton text={code} size="icon-sm" copiedTooltip={`Copied ${size}`} copiedDuration={1500} />
          <span aria-hidden className="bg-border mx-1 h-4 w-px" />
          <Kbd>Esc</Kbd>
          <DialogClose asChild>
            <TooltipIconButton tooltip="Close" size="icon-sm">
              <X />
            </TooltipIconButton>
          </DialogClose>
        </div>
        <pre
          ref={preRef}
          tabIndex={-1}
          className={cn(
            "bg-code-surface text-code min-h-0 flex-1 py-3 font-mono text-sm leading-relaxed outline-none",
            wrap ? "overflow-x-hidden overflow-y-auto" : "overflow-auto",
          )}
          style={sideFade(more, "var(--spacing-code-gutter-lg)")}
        >
          <Lines lines={lines} gutter="dialog" wrap={wrap} />
        </pre>
      </DialogContent>
    </Dialog>
  );
};

/** A mermaid diagram once it parses (while it streams, its code), with Expand and options. */
const Diagram: FC<{ code: string }> = ({ code }) => {
  const [svg, setSvg] = useState<{ code: string; markup: string | null } | null>(null);
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    let live = true;
    const timer = window.setTimeout(() => {
      import("@/lib/mermaid")
        .then(({ renderDiagram }) => renderDiagram(code))
        .then((markup) => live && setSvg({ code, markup }))
        .catch(() => live && setSvg({ code, markup: null }));
    }, 200);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [code]);
  const markup = svg?.code === code ? svg.markup : null;
  const drawn = (
    // Mermaid's strict mode sanitizes the SVG it returns.
    // oxlint-disable-next-line react/no-danger
    <div className="[&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-none" dangerouslySetInnerHTML={{ __html: markup ?? "" }} />
  );
  return (
    <div data-slot="diagram" className="group/diagram relative my-3 first:mt-0 last:mb-0">
      {markup ? (
        <div className="overflow-x-auto py-2">{drawn}</div>
      ) : (
        <pre className="text-code bg-code-surface border-border overflow-x-auto rounded-2xl border p-3 font-mono text-sm">
          {code}
        </pre>
      )}
      {markup && (
        <div className="bg-background/80 absolute end-1 top-1 flex rounded-control opacity-0 transition-opacity group-hover/diagram:opacity-100 focus-within:opacity-100">
          <TooltipIconButton tooltip="Expand diagram" size="icon-xs" onClick={() => setExpanded(true)}>
            <ExpandLg />
          </TooltipIconButton>
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <TooltipIconButton tooltip="Diagram options" size="icon-xs">
                <DotsHorizontal />
              </TooltipIconButton>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(code)}>
                Copy code
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(markup)}>
                Copy SVG
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      )}
      <Dialog open={expanded} onOpenChange={setExpanded}>
        <DialogContent className="max-h-9/10 w-9/10 max-w-none overflow-auto">
          <DialogTitle className="sr-only">Diagram</DialogTitle>
          {drawn}
        </DialogContent>
      </Dialog>
    </div>
  );
};

/** The header is part of the card `CodeBlock` draws. */
export const CodeHeader: FC = () => null;
