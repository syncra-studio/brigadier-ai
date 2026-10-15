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
import { Check, Copy, ExpandLg, Globe, Mail } from "@openai/apps-sdk-ui/components/Icon";

import { CodeBlock, CodeHeader } from "@/components/assistant-ui/code-block";
import {
  InlineImageContext,
  LinkTimeContext,
  OpenFileContext,
  TaskMentionContext,
  useCheckoutRoot,
  useLinkConversation,
} from "@/components/assistant-ui/file-links";
import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { ImagePreview } from "@/components/assistant-ui/elements/image-viewer";
import { codePath, type CodePath, underRoot, useCodePathExists } from "@/components/assistant-ui/code-paths";
import { httpsOrigin, useSiteIcon } from "@/components/assistant-ui/site-icons";
import { createWordFade } from "@/components/assistant-ui/word-fade";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { openArtifact, openFolder, openUrl, request, revealPath, saveArtifact, saveCopy } from "@/ipc/client";
import type { StoredCopy } from "@/ipc/generated";
import { openLink } from "@/state/links";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

type MarkdownTextProps = Partial<TextMessagePartProps> & {
  components?: Parameters<typeof memoizeMarkdownComponents>[0];
  /** The text is still streaming: it shows at a steady pace and each word fades in. */
  streaming?: boolean | undefined;
  /**
   * Streaming text shows at `STREAM_PACE` (the default). Off, it shows as it arrives, each word
   * still fading in: a line between the turn's steps is never held back.
   */
  paced?: boolean | undefined;
  /**
   * The user's own message: each line break is a break, as they typed it, and in its text (never
   * in code) a `@task-N` is a `brigadier-task:N` link and a pasted `[Image #n]` a
   * `brigadier-image:n` link, which `TaskMentionContext` and `InlineImageContext` show.
   */
  breaks?: boolean | undefined;
};

type MdNode = { type: string; value?: string; url?: string; children?: MdNode[] };

/** A worker's `@task-N`, or a pasted image's `[Image #n]`. */
const USER_TOKEN = /(@task-\d+\b|\[Image #\d+\])/;

/** One piece of the user's text: a line break, a mention or an image as a link, else text. */
function userPieces(value: string): MdNode[] {
  return value.split(/\r?\n/).flatMap((line, index) => [
    ...(index > 0 ? [{ type: "break" }] : []),
    ...line.split(USER_TOKEN).flatMap((piece, at): MdNode[] => {
      if (!piece) return [];
      if (at % 2 === 0) return [{ type: "text", value: piece }];
      const url = piece.startsWith("@") ? `brigadier-task:${piece.slice(6)}` : `brigadier-image:${piece.slice(8, -1)}`;
      return [{ type: "link", url, children: [{ type: "text", value: piece }] }];
    }),
  ]);
}

/**
 * The user's text in a paragraph: its line breaks as breaks, its mentions and images as links
 * (code keeps its own lines and words, a link's words stay its own).
 */
function splitUserText(node: MdNode): void {
  if (!node.children) return;
  // A `[Image #n]` may come in several text pieces (its brackets on their own).
  const merged: MdNode[] = [];
  for (const child of node.children) {
    const last = merged.at(-1);
    if (child.type === "text" && last?.type === "text") last.value = `${last.value ?? ""}${child.value ?? ""}`;
    else merged.push(child.type === "text" ? { ...child } : child);
  }
  node.children = merged.flatMap((child): MdNode[] => {
    if (child.type === "text") return userPieces(child.value ?? "");
    if (child.type !== "link" && child.type !== "linkReference") splitUserText(child);
    return [child];
  });
}

const remarkUserText = () => (tree: MdNode) => splitUserText(tree);

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

const MarkdownTextImpl: FC<MarkdownTextProps> = ({ components, streaming, paced = true, breaks = false }) => {
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
      remarkPlugins={breaks ? [remarkGfm, remarkUserText] : [remarkGfm]}
      rehypePlugins={rehypePlugins}
      // Only text that still streams is revealed as it arrives. Settled text shows whole at
      // once, also when it mounts again (the stored reply replacing the streamed one, a work
      // block folding): revealed from nothing, it would shrink the thread for a moment and
      // pull it off the newest message. The end of a stream shows the rest at once.
      smooth={streaming === true && paced && STREAM_PACE}
      className="aui-md font-prose"
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

/** What a click on a file link found when its file was gone, shown after the chip. */
type Gone = { stored: StoredCopy | null };

/**
 * The file-link chip: the file's type icon and its name in link blue, "(line 36)"
 * after it, the absolute path on hover. A click opens the file in the Files tab, or shows it
 * in Finder when it isn't the session's. A worker's file whose scratch folder is gone opens
 * the copy Brigadier stored with its report (BUILD-PLAN A1), with "This file is no longer
 * here" and that copy's Save after the chip; never an error toast.
 */
export const FileChip: FC<{ href: string; children: ReactNode }> = ({ href, children }) => {
  const root = useCheckoutRoot();
  const openFile = useContext(OpenFileContext);
  const conversationId = useLinkConversation();
  const atMs = useContext(LinkTimeContext);
  const [gone, setGone] = useState<Gone | null>(null);
  const { path, line } = fileTarget(href, root);
  const open = async () => {
    const found = conversationId
      ? await request({
          method: "resolveFileLink",
          conversationId,
          link: href,
          path,
          ...(atMs === null ? {} : { atMs }),
        }).catch(() => null)
      : null;
    if (found && !found.exists) {
      setGone({ stored: found.stored });
      if (found.stored) {
        const { artifact } = found.stored;
        await openArtifact(artifact.id, artifact.fileName ?? artifact.title);
      }
      return;
    }
    setGone(null);
    if (openFile(path, line)) return;
    await revealPath(path).catch((error: unknown) => {
      // Gone, with nothing stored (no session to ask, or not a worker's file).
      if (found) throw error;
      setGone({ stored: null });
    });
  };
  return (
    <>
      <ContextMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <ContextMenuTrigger asChild>
          <a
            href={href}
            data-slot="file-chip"
            onClick={(event) => {
              event.preventDefault();
              open().catch(failed);
            }}
            className="aui-md-file text-link hover:text-link/80 cursor-pointer no-underline"
          >
            <FileTypeIcon name={path} className="me-0.5 inline-block size-icon-sm align-text-bottom" />
            {children}
            {line !== null && <span className="text-muted-foreground"> (line {line})</span>}
          </a>
          </ContextMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">{path}</TooltipContent>
      </Tooltip>
      <FileChipMenu path={path} line={line} />
      </ContextMenu>
      {gone && (
        <span data-slot="file-chip-gone" className="text-muted-foreground">
          {" "}
          (This file is no longer here
          {gone.stored && (
            <>
              {"; Brigadier kept a copy: "}
              <button
                type="button"
                data-slot="file-chip-save"
                onClick={() => {
                  const { artifact } = gone.stored!;
                  saveArtifact(artifact.id, artifact.fileName ?? artifact.title).catch(failed);
                }}
                className="text-link hover:text-link/80 cursor-pointer"
              >
                Save…
              </button>
            </>
          )}
          )
        </span>
      )}
    </>
  );
};

/**
 * A file chip's right-click menu (inside its `ContextMenu`): open it in the Files tab, its default app or Finder; show it in
 * Finder; copy its path; save a copy to Downloads.
 */
function FileChipMenu({ path, line }: { path: string; line: number | null }) {
  const openFile = useContext(OpenFileContext);
  const mac = useApp((s) => s.info?.platform === "macos");
  const finder = mac ? "Finder" : "File manager";
  const { copyToClipboard } = useCopyToClipboard();
  const download = () => {
    saveCopy({ name: path.split("/").pop() || "file", path, ask: false })
      .then(() => toast("Saved a copy to Downloads"))
      .catch(() => toast("Could not download a copy", { tone: "error" }));
  };
  return (
      <ContextMenuContent data-slot="file-chip-menu">
        <ContextMenuSub>
          <ContextMenuSubTrigger>Open in</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem onSelect={() => openFile(path, line)}>Files tab</ContextMenuItem>
            <ContextMenuItem onSelect={() => openFolder(path).catch(failed)}>Default app</ContextMenuItem>
            <ContextMenuItem onSelect={() => revealPath(path).catch(failed)}>{finder}</ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem onSelect={() => revealPath(path).catch(failed)}>{mac ? "Show in Finder" : "Show in file manager"}</ContextMenuItem>
        <ContextMenuItem onSelect={() => copyToClipboard(path)}>Copy path</ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={download}>Download a copy</ContextMenuItem>
      </ContextMenuContent>
  );
}

/** A web link's right-click menu (inside its `ContextMenu`): open it in the system's browser, or copy it. */
function LinkMenu({ href }: { href: string }) {
  const { copyToClipboard } = useCopyToClipboard();
  const web = /^https?:/i.test(href);
  return (
      <ContextMenuContent data-slot="link-menu">
        {web && <ContextMenuItem onSelect={() => openUrl(href).catch(failed)}>Open in external browser</ContextMenuItem>}
        <ContextMenuItem onSelect={() => copyToClipboard(web ? href : href.replace(/^mailto:/i, ""))}>
          {web ? "Copy link" : "Copy email address"}
        </ContextMenuItem>
      </ContextMenuContent>
  );
}

/** Web links open where Settings › Browser say; anything else that isn't a page is a file. */
export const MarkdownLink: FC<React.ComponentProps<"a">> = ({ className, href, children, ...props }) => {
  const task = useContext(TaskMentionContext);
  const image = useContext(InlineImageContext);
  // A worker the user's message @-mentions: its chip. An image pasted into it: its chip, there.
  if (href?.startsWith("brigadier-task:")) return <>{task?.(Number(href.slice("brigadier-task:".length))) ?? children}</>;
  if (href?.startsWith("brigadier-image:")) return <>{image?.(Number(href.slice("brigadier-image:".length))) ?? children}</>;
  if (href && !/^(https?:|mailto:|#)/i.test(href)) {
    return <FileChip href={href}>{children}</FileChip>;
  }
  const mail = !!href && /^mailto:/i.test(href);
  const link = (
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
          openLink(href, { agent: true }).catch(failed);
        } else if (mail) {
          // A new message in the user's mail app, for them to send or not.
          openUrl(href).catch(failed);
        }
      }}
      className={cn(
        "aui-md-a text-link hover:text-link/80 font-medium underline-offset-2 hover:underline hover:decoration-dashed hover:decoration-0.5",
        className,
      )}
    >
      {/* The icon stays on its first word's line: a word joiner holds them together. */}
      {href && httpsOrigin(href) && <SiteIcon href={href} />}
      {mail && <Mail data-slot="site-icon" data-mail aria-hidden className="me-0.75 inline-block size-icon-sm align-text-bottom select-none" />}
      {href && (httpsOrigin(href) || mail) && <span className="select-none">{"\u2060"}</span>}
      {children}
    </a>
  );
  if (!href || href.startsWith("#")) return link;
  // Its address on hover, and a right-click menu.
  return (
    <ContextMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <ContextMenuTrigger asChild>{link}</ContextMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top" data-slot="link-address" className="max-w-sm break-all">
          {mail ? href.replace(/^mailto:/i, "") : href}
        </TooltipContent>
      </Tooltip>
      <LinkMenu href={href} />
    </ContextMenu>
  );
};

/**
 * A web link's site icon, 16px before its words: a globe until the daemon has the icon, and when
 * the site has none. Left out of copied text.
 */
function SiteIcon({ href }: { href: string }) {
  const icon = useSiteIcon(href);
  return icon ? (
    <img data-slot="site-icon" src={icon} alt="" referrerPolicy="no-referrer" className="me-0.75 inline-block size-icon-sm rounded-xs object-contain align-text-bottom select-none" />
  ) : (
    <Globe data-slot="site-icon" aria-hidden className="me-0.75 inline-block size-icon-sm align-text-bottom select-none" />
  );
}

/**
 * Inline code that names a file of the session's checkout (`src/cli.ts:64`): a file chip
 * opening it at that line, once the file is known to exist; plain code until then and when not.
 */
const CodeFile: FC<React.ComponentProps<"code"> & { named: CodePath }> = ({ named, className, ...props }) => {
  const root = useCheckoutRoot();
  const conversationId = useLinkConversation();
  // The checkout may change with each message and whenever the thread starts or stops working.
  const updated = useApp((s) => (conversationId ? (s.conversations[conversationId]?.updatedAtMs ?? 0) : 0));
  const run = useBoard((s) => s.board?.run ?? "idle");
  const exists = useCodePathExists(root ? conversationId : null, root, `${updated}:${run}`, named);
  if (exists && root) {
    const href = `${underRoot(root, named.path)}${named.line === null ? "" : `:${named.line}`}`;
    return <FileChip href={href}>{named.path}</FileChip>;
  }
  return <code className={cn("aui-md-inline-code bg-muted text-code-inline rounded-md px-1.5 py-0.5 font-mono", className)} {...props} />;
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
  a: MarkdownLink,
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
  // An image in an answer opens in the image viewer.
  img: ({ src, alt, className }) =>
    typeof src === "string" && src ? (
      <ImagePreview url={src} name={alt || "Image"}>
        <button type="button" aria-label={`View ${alt || "image"}`} className="aui-md-img my-3 block cursor-zoom-in">
          <img src={src} alt={alt ?? ""} className={cn("max-h-80 max-w-full rounded-lg", className)} />
        </button>
      </ImagePreview>
    ) : null,
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
    const named = !isCodeBlock && typeof props.children === "string" ? codePath(props.children) : null;
    if (named) return <CodeFile named={named} className={className} {...props} />;
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
