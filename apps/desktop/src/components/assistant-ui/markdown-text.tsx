import {
  MarkdownTextPrimitive,
  unstable_memoizeMarkdownComponents as memoizeMarkdownComponents,
  useIsMarkdownCodeBlock,
} from "@assistant-ui/react-markdown";
import remarkGfm from "remark-gfm";
import {
  type FC,
  memo,
  type ReactNode,
  useContext,
  useMemo,
  useRef,
  useState,
} from "react";
import type { TextMessagePartProps } from "@assistant-ui/react";
import { Check, Copy, ExpandLg } from "@openai/apps-sdk-ui/components/Icon";

import { CodeBlock, CodeHeader } from "@/components/assistant-ui/code-block";
import { OpenFileContext, useCheckoutRoot } from "@/components/assistant-ui/file-links";
import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { createWordFade } from "@/components/assistant-ui/word-fade";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { openUrl, revealPath } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { toast } from "@/state/toasts";

type MarkdownTextProps = Partial<TextMessagePartProps> & {
  components?: Parameters<typeof memoizeMarkdownComponents>[0];
  /** The text is still streaming: it shows at a steady pace and each word fades in. */
  streaming?: boolean | undefined;
};

/**
 * The pace streaming text shows at: it trails what has arrived, steered so whatever waits
 * shows in about a second (never slower than a character a second), and the shown text
 * updates at most every 50ms. Off when the system asks for less motion.
 */
const STREAM_PACE = { drainMs: 1000, maxCharIntervalMs: 1000, minCommitMs: 50 };

/**
 * Keeps file links like `notes.py:36`, which react-markdown's default would drop as an
 * unknown scheme. Links never navigate the app (web links open in the browser, the rest are
 * file chips), so only script-bearing schemes are removed.
 */
const keepLinks = (url: string) => (/^\s*(javascript|vbscript|data):/i.test(url) ? "" : url);

const shallowEqual = (
  a: Record<string, unknown> | undefined,
  b: Record<string, unknown> | undefined,
) =>
  a !== undefined &&
  b !== undefined &&
  Object.keys(a).length === Object.keys(b).length &&
  Object.keys(b).every((key) => a[key] === b[key]);

// Keeps the previous object while a new one is shallow-equal to it, so inline
// `components` props don't rebuild the memoized markdown components each render.
const useShallowStable = <T extends Record<string, unknown> | undefined>(
  value: T,
): T => {
  const [stable, setStable] = useState(value);
  if (value !== stable && !shallowEqual(stable, value)) {
    setStable(value);
    return value;
  }
  return stable;
};

const MarkdownTextImpl: FC<MarkdownTextProps> = ({ components, streaming }) => {
  const stableComponents = useShallowStable(components);
  const markdownComponents = useMemo(() => {
    if (!stableComponents) return defaultComponents;
    return {
      ...defaultComponents,
      ...memoizeMarkdownComponents(stableComponents),
    };
  }, [stableComponents]);
  // One per streaming text: it remembers which of its words already faded in.
  const rehypePlugins = useMemo(() => (streaming ? [createWordFade()] : undefined), [streaming]);

  return (
    <MarkdownTextPrimitive
      remarkPlugins={[remarkGfm]}
      rehypePlugins={rehypePlugins}
      // Only text that still streams is revealed as it arrives. Settled text shows whole at
      // once, also when it mounts again (the stored reply replacing the streamed one, a work
      // block folding): revealed from nothing, it would shrink the thread for a moment and
      // pull it off the newest message. The end of a stream shows the rest at once.
      smooth={streaming === true && STREAM_PACE}
      className="aui-md"
      urlTransform={keepLinks}
      components={markdownComponents}
      defer
    />
  );
};

export const MarkdownText = memo(MarkdownTextImpl);

function failed(error: unknown): void {
  toast(error instanceof Error ? error.message : String(error), { tone: "error" });
}

/** A link to a file: its absolute path and line, from `path`, `path:36`, `path#L36`, `file://…`. */
function fileTarget(href: string, root: string | null): { path: string; line: number | null } {
  let path = href.startsWith("file://") ? href.slice("file://".length) : href;
  try {
    path = decodeURIComponent(path);
  } catch {
    // Keep it as written.
  }
  const found = /(?::|#L)(\d+)(?:[-:]L?\d+)?$/.exec(path);
  const line = found ? Number(found[1]) : null;
  if (found) path = path.slice(0, found.index);
  if (!path.startsWith("/") && root) path = `${root.replace(/\/$/, "")}/${path.replace(/^\.\//, "")}`;
  return { path, line };
}

/**
 * The file-link chip: the file's type icon and its name in link blue, "(line 36)"
 * after it, the absolute path on hover. A click opens the file in the Files tab, or shows it
 * in Finder when it isn't the session's.
 */
const FileChip: FC<{ href: string; children: ReactNode }> = ({ href, children }) => {
  const root = useCheckoutRoot();
  const openFile = useContext(OpenFileContext);
  const { path, line } = fileTarget(href, root);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <a
          href={href}
          data-slot="file-chip"
          onClick={(event) => {
            event.preventDefault();
            if (!openFile(path, line)) revealPath(path).catch(failed);
          }}
          className="aui-md-file text-link hover:text-link/80 cursor-pointer no-underline"
        >
          <FileTypeIcon name={path} className="me-0.5 inline-block size-icon-sm align-text-bottom" />
          {children}
          {line !== null && <span className="text-muted-foreground"> (line {line})</span>}
        </a>
      </TooltipTrigger>
      <TooltipContent side="top">{path}</TooltipContent>
    </Tooltip>
  );
};

/** Web links open in the browser; anything else that isn't a page is a file. */
const Link: FC<React.ComponentProps<"a">> = ({ className, href, children, ...props }) => {
  if (href && !/^(https?:|mailto:|#)/i.test(href)) {
    return <FileChip href={href}>{children}</FileChip>;
  }
  return (
    <a
      {...props}
      href={href}
      onClick={(event) => {
        event.preventDefault();
        if (!href) return;
        if (href.startsWith("#")) {
          // A link within the answer: its target, when the answer has one by that id.
          document.getElementById(decodeURIComponent(href.slice(1)))?.scrollIntoView({ block: "start" });
        } else if (/^https?:/i.test(href)) {
          // Only web pages: a model's link must not launch other handlers (mailto: included).
          openUrl(href).catch(failed);
        }
      }}
      className={cn(
        "aui-md-a text-link hover:text-link/80 underline-offset-2 hover:underline",
        className,
      )}
    >
      {children}
    </a>
  );
};

const line = (cells: string[]) => `| ${cells.join(" | ")} |`;

/** A table's text as a Markdown table, for "Copy table". */
function tableMarkdown(table: HTMLTableElement): string {
  const rows = [...table.rows].map((row) =>
    [...row.cells].map((cell) => (cell.textContent ?? "").trim().replaceAll("|", "\\|")),
  );
  const [head, ...body] = rows;
  if (!head) return "";
  return [line(head), line(head.map(() => "---")), ...body.map(line)].join("\n");
}

/** The table: rules between rows, no outer border; on hover "Expand table" and "Copy table". */
const Table: FC<React.ComponentProps<"table">> = ({ className, ...props }) => {
  const ref = useRef<HTMLTableElement>(null);
  const [expanded, setExpanded] = useState(false);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const table = (
    <table
      className={cn("aui-md-table w-full border-collapse text-sm", className)}
      {...props}
    />
  );
  return (
    <div className="aui-md-table-wrapper group/table relative my-3 first:mt-0 last:mb-0">
      <div className="overflow-x-auto">
        <table ref={ref} className={cn("aui-md-table w-full border-collapse text-sm", className)} {...props} />
      </div>
      <div className="bg-background/80 rounded-control absolute end-0 -top-1 flex opacity-0 transition-opacity group-hover/table:opacity-100 focus-within:opacity-100">
        <TooltipIconButton tooltip="Expand table" size="icon-xs" onClick={() => setExpanded(true)}>
          <ExpandLg />
        </TooltipIconButton>
        <TooltipIconButton
          tooltip={isCopied ? "Copied" : "Copy table"}
          size="icon-xs"
          onClick={() => ref.current && copyToClipboard(tableMarkdown(ref.current))}
        >
          {isCopied ? <Check /> : <Copy />}
        </TooltipIconButton>
      </div>
      <Dialog open={expanded} onOpenChange={setExpanded}>
        <DialogContent className="max-h-9/10 w-9/10 max-w-none overflow-auto">
          <DialogTitle className="sr-only">Table</DialogTitle>
          {table}
        </DialogContent>
      </Dialog>
    </div>
  );
};

const defaultComponents = memoizeMarkdownComponents({
  h1: ({ className, ...props }) => (
    <h1
      className={cn(
        "aui-md-h1 mt-5 mb-2 scroll-m-20 text-xl font-semibold first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h2: ({ className, ...props }) => (
    <h2
      className={cn(
        "aui-md-h2 mt-5 mb-2 scroll-m-20 text-lg font-semibold first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h3: ({ className, ...props }) => (
    <h3
      className={cn(
        "aui-md-h3 mt-4 mb-1.5 scroll-m-20 text-base font-semibold first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h4: ({ className, ...props }) => (
    <h4
      className={cn(
        "aui-md-h4 mt-3.5 mb-1 scroll-m-20 text-base font-medium first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h5: ({ className, ...props }) => (
    <h5
      className={cn(
        "aui-md-h5 mt-3 mb-1 text-sm font-semibold first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h6: ({ className, ...props }) => (
    <h6
      className={cn(
        "aui-md-h6 mt-3 mb-1 text-sm font-medium first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  p: ({ className, ...props }) => (
    <p
      className={cn(
        "aui-md-p my-3 leading-relaxed first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  a: Link,
  blockquote: ({ className, ...props }) => (
    <blockquote
      className={cn(
        "aui-md-blockquote border-muted-foreground/30 text-muted-foreground my-3 border-s-2 ps-4",
        className,
      )}
      {...props}
    />
  ),
  ul: ({ className, ...props }) => (
    <ul
      className={cn(
        "aui-md-ul marker:text-muted-foreground my-3 ms-5 list-disc [&>li]:mt-1",
        className,
      )}
      {...props}
    />
  ),
  ol: ({ className, ...props }) => (
    <ol
      className={cn(
        "aui-md-ol marker:text-muted-foreground my-3 ms-5 list-decimal [&>li]:mt-1",
        className,
      )}
      {...props}
    />
  ),
  hr: ({ className, ...props }) => (
    <hr
      className={cn("aui-md-hr border-muted-foreground/20 my-3", className)}
      {...props}
    />
  ),
  table: Table,
  th: ({ className, ...props }) => (
    <th
      className={cn(
        "aui-md-th border-border border-b px-3 py-2 text-start font-semibold [[align=center]]:text-center [[align=right]]:text-right",
        className,
      )}
      {...props}
    />
  ),
  td: ({ className, ...props }) => (
    <td
      className={cn(
        "aui-md-td px-3 py-2 text-start align-top [[align=center]]:text-center [[align=right]]:text-right",
        className,
      )}
      {...props}
    />
  ),
  tr: ({ className, ...props }) => (
    <tr
      className={cn("aui-md-tr border-border/60 border-b last:border-b-0", className)}
      {...props}
    />
  ),
  li: ({ className, ...props }) => (
    <li className={cn("aui-md-li leading-relaxed", className)} {...props} />
  ),
  strong: ({ className, ...props }) => (
    <strong
      className={cn("aui-md-strong font-semibold", className)}
      {...props}
    />
  ),
  sup: ({ className, ...props }) => (
    <sup
      className={cn("aui-md-sup [&>a]:text-xs [&>a]:no-underline", className)}
      {...props}
    />
  ),
  pre: ({ className, ...props }) => (
    <pre
      className={cn(
        "aui-md-pre border-border/50 bg-code-surface text-code overflow-x-auto rounded-t-none rounded-b-xl border border-t-0 p-3.5 font-mono",
        className,
      )}
      {...props}
    />
  ),
  code: function Code({ className, ...props }) {
    const isCodeBlock = useIsMarkdownCodeBlock();
    return (
      <code
        className={cn(
          !isCodeBlock &&
            "aui-md-inline-code bg-muted text-code-inline rounded-md px-1.5 py-0.5 font-mono",
          className,
        )}
        {...props}
      />
    );
  },
  CodeHeader,
  SyntaxHighlighter: CodeBlock,
});
