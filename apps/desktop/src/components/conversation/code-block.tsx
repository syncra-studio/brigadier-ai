import type { SyntaxHighlighterProps } from "@assistant-ui/react-markdown";
import { type FC, type ReactNode, type RefObject, useEffect, useLayoutEffect, useRef, useState } from "react";

import { CopyButton } from "@/components/shared/copy-button";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { Dialog, DialogContent, DialogTitle } from "@/components/overlays/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays/dropdown-menu";
import { ArrowRight, Code, DotsHorizontal, ExpandLg } from "@/components/icons";
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

/** A diff's line in colour: + green, − red, @@ blue, file headers grey italic. */
function diffLineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return "text-muted-foreground italic";
  if (line.startsWith("+")) return "text-success";
  if (line.startsWith("-")) return "text-destructive";
  if (line.startsWith("@@")) return "text-link";
  return "";
}

const Body: FC<{ code: string; language: string }> = ({ code, language }) => {
  const tokens = useTokens(code, language);
  if (isDiff(language)) {
    return (
      <>
        {code.split("\n").map((line, index) => (
          // oxlint-disable-next-line react/no-array-index-key -- lines have no identity
          <span key={index} className={cn("block", diffLineClass(line))}>
            {line || " "}
          </span>
        ))}
      </>
    );
  }
  if (!tokens) return <>{code}</>;
  return (
    <>
      {tokens.map((line, index) => (
        // oxlint-disable-next-line react/no-array-index-key -- lines have no identity
        <span key={index} className="block">
          {line.length === 0
            ? " "
            : line.map((token, at) => (
                <span
                  // oxlint-disable-next-line react/no-array-index-key -- tokens have no identity
                  key={at}
                  style={{ color: token.color }}
                  className={cn(token.italic && "italic")}
                >
                  {token.content}
                </span>
              ))}
        </span>
      ))}
    </>
  );
};

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

const Card: FC<{ header: ReactNode; children: ReactNode }> = ({ header, children }) => (
  <div
    data-slot="code-block"
    className="border-border bg-code-surface my-3 overflow-hidden rounded-2xl border first:mt-0 last:mb-0"
  >
    <div className="text-muted-foreground flex h-control-md items-center gap-1.5 ps-3 pe-1.5 text-xs">
      {header}
    </div>
    {children}
  </div>
);

/**
 * The code block: a card with `</>` and the language on the left, "Enable word wrap"
 * and Copy on the right, the code syntax-highlighted (a diff in its own colours). Lines longer
 * than the card scroll sideways, the side with more fading out (the scrollbar only shows while
 * scrolling). A mermaid block draws its diagram.
 */
export const CodeBlock: FC<SyntaxHighlighterProps> = ({ code, language }) => {
  const [wrap, setWrap] = useState(false);
  const preRef = useRef<HTMLPreElement>(null);
  const more = useMoreSideways(preRef, !wrap && language.toLowerCase() !== "mermaid");
  if (language.toLowerCase() === "mermaid") return <Diagram code={code} />;
  return (
    <Card
      header={
        <>
          <Code aria-hidden className="size-icon-sm" />
          <span className="flex-1 font-medium">{languageName(language)}</span>
          <TooltipIconButton
            tooltip={wrap ? "Disable word wrap" : "Enable word wrap"}
            size="icon-xs"
            aria-pressed={wrap}
            className={cn(wrap && "text-foreground")}
            onClick={() => setWrap(!wrap)}
          >
            <ArrowRight />
          </TooltipIconButton>
          <CopyButton text={code} size="icon-xs" />
        </>
      }
    >
      <pre
        ref={preRef}
        data-more={more.start && more.end ? "both" : more.start ? "start" : more.end ? "end" : undefined}
        className={cn(
          "text-code px-3 pb-3 font-mono text-sm leading-relaxed",
          wrap ? "break-words whitespace-pre-wrap" : "overflow-x-auto",
        )}
        style={
          more.start || more.end
            ? {
                maskImage: `linear-gradient(to right, ${more.start ? "transparent" : "black"}, black ${FADE}, black calc(100% - ${FADE}), ${more.end ? "transparent" : "black"})`,
              }
            : undefined
        }
      >
        <code>
          <Body code={code} language={language} />
        </code>
      </pre>
    </Card>
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
