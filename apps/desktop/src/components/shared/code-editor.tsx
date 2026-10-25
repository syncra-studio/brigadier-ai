import { type ComponentType, type ReactNode, useEffect, useMemo, useState } from "react";

import { FileTypeIcon } from "@/components/conversation";
import { TooltipIconButton } from "@/components/controls";
import { Check, Copy, FolderOpen } from "@/components/icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { revealPath } from "@/ipc/client";
import { HIGHLIGHT_CHARS, highlight, type Token } from "@/lib/highlight";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/*
 * A text file as the thread's file tab shows it: a header with the file's icon, name, folder,
 * what sits at its end (its size) and Copy path / Reveal, then its lines, numbered and coloured.
 * `CodeEditor` draws the lines of a small file whole, read only or editable; the file tab keeps
 * its own virtualized lines for files of any size.
 */

const baseName = (path: string) => path.slice(path.lastIndexOf("/") + 1);
const dirName = (path: string) => {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
};

/** A file's header: icon, name and folder, `children` (its size, a review's bar), Copy path and Reveal. */
export function CodeFileHeader({
  path,
  shown = path,
  reveal = path,
  gone = false,
  heading: Heading = "h2",
  actions,
  children,
}: {
  /** What Copy path copies. */
  path: string;
  /** The path the header names: its last part bold, the rest grey. */
  shown?: string;
  /** The absolute path Reveal opens; null leaves Reveal out. */
  reveal?: string | null;
  /** The file is gone: Reveal is off. */
  gone?: boolean;
  /** The name's element: a dialog's title in a dialog. */
  heading?: "h2" | ComponentType<{ className?: string; title?: string; children: ReactNode }>;
  /** After Reveal: a dialog's close. */
  actions?: ReactNode;
  children?: ReactNode;
}) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const mac = useApp((s) => s.info?.platform === "macos");
  return (
    <header data-slot="code-file-header" className="border-border flex h-control-lg shrink-0 items-center gap-2 border-b px-4">
      <FileTypeIcon name={shown} className="size-icon-md shrink-0" />
      <Heading className="min-w-0 flex-1 truncate text-sm" title={reveal ?? path}>
        <span className="font-medium">{baseName(shown)}</span>
        {dirName(shown) && <span className="text-muted-foreground"> {dirName(shown)}</span>}
      </Heading>
      {children}
      <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy path"} size="icon-sm" onClick={() => copyToClipboard(path)}>
        {isCopied ? <Check /> : <Copy />}
      </TooltipIconButton>
      {reveal !== null && (
        <TooltipIconButton
          tooltip={mac ? "Reveal in Finder" : "Open in File Manager"}
          size="icon-sm"
          disabled={gone}
          onClick={() => revealPath(reveal).catch((cause: unknown) => toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" }))}
        >
          <FolderOpen />
        </TooltipIconButton>
      )}
      {actions}
    </header>
  );
}

/** `text`'s lines as coloured pieces, once highlighted; null until then, or for a language without colours. */
function useTokens(text: string, language: string): Token[][] | null {
  const [tokens, setTokens] = useState<{ text: string; language: string; tokens: Token[][] | null } | null>(null);
  useEffect(() => {
    if (text.length > HIGHLIGHT_CHARS) return;
    let live = true;
    // Each edit colours afresh; until it lands, the lines it changed show plain.
    highlight(text, language)
      .then((result) => live && setTokens({ text, language, tokens: result }))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [text, language]);
  return tokens?.language === language ? tokens.tokens : null;
}

/**
 * A small file's lines, numbered and coloured like the file tab's: read only, or with `onChange`
 * editable in place. ⌘S (Ctrl+S) calls `onSave` while it has focus.
 */
export function CodeEditor({
  value,
  language,
  label,
  onChange,
  onSave,
  autoFocus = false,
  className,
}: {
  value: string;
  /** Its language for colours (`languageOf(path)`). */
  language: string;
  /** The accessible name of the text. */
  label: string;
  onChange?: (value: string) => void;
  onSave?: () => void;
  /** An editable one takes focus as it shows. */
  autoFocus?: boolean;
  className?: string;
}) {
  const editable = onChange !== undefined;
  // Read only, a last newline ends the last line (as in the file tab); editing, it starts a new one.
  const shown = editable ? value : value.replace(/\n$/, "");
  const lines = useMemo(() => shown.split("\n"), [shown]);
  const tokens = useTokens(shown, language);
  const mac = useApp((s) => s.info?.platform === "macos");
  const gutter = `${String(lines.length).length}ch`;
  const pieces = (index: number): Token[] => {
    const line = lines[index] ?? "";
    const coloured = tokens?.[index];
    // A line edited since the colours came keeps its text plain until they come again.
    return coloured && coloured.map((piece) => piece.content).join("") === line ? coloured : [{ content: line, color: undefined, italic: false }];
  };
  return (
    <div data-slot="code-editor" data-selectable={editable ? undefined : true} className={cn("min-h-0 flex-1 overflow-auto py-2 font-mono text-xs", className)}>
      <div className="flex w-max min-w-full">
        <div aria-hidden className="text-muted-foreground shrink-0 ps-4 pe-4 text-end leading-5 select-none" style={{ width: `calc(${gutter} + var(--spacing) * 8)` }}>
          {lines.map((_, index) => (
            // Line numbers are their places.
            // oxlint-disable-next-line react/no-array-index-key
            <div key={index} className="h-5">{index + 1}</div>
          ))}
        </div>
        <div className="relative min-w-0 flex-1">
          <div aria-hidden={editable || undefined} role={editable ? undefined : "textbox"} aria-readonly={editable ? undefined : true} aria-label={editable ? undefined : label} aria-multiline={editable ? undefined : true} className="pe-4 leading-5 whitespace-pre">
            {lines.map((_, index) => (
              // Lines are their places in the text.
              // oxlint-disable-next-line react/no-array-index-key
              <div key={index} data-line={index + 1} className="h-5">
                {pieces(index).map((piece, at) => (
                  // Pieces of a line never reorder.
                  // oxlint-disable-next-line react/no-array-index-key
                  <span key={at} className={piece.italic ? "italic" : undefined} style={piece.color ? { color: piece.color } : undefined}>{piece.content}</span>
                ))}
              </div>
            ))}
          </div>
          {editable && (
            <textarea
              aria-label={label}
              autoFocus={autoFocus}
              value={value}
              wrap="off"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(event) => onChange(event.target.value)}
              onKeyDown={(event) => {
                const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
                if (command && !event.altKey && !event.shiftKey && event.code === "KeyS") {
                  event.preventDefault();
                  onSave?.();
                }
              }}
              className="caret-foreground absolute inset-0 resize-none overflow-hidden bg-transparent pe-4 font-mono text-xs leading-5 whitespace-pre text-transparent outline-none"
            />
          )}
        </div>
      </div>
    </div>
  );
}
