import { useAui, useAuiState } from "@assistant-ui/react";
import type { LexicalEditor } from "lexical";
import { createContext, type MouseEventHandler, useCallback, useContext, useEffect, useRef, useState } from "react";

import { literalMarkdown } from "@/components/conversation/composerMarkdown";
import { PageEditor } from "@/components/views/page-editor/PageEditor";
import "@/app/conversation/pages/PageTab.css";
import {
  AttachmentReaderContext,
  type AttachmentSource,
  pastedTextFile,
  PasteShortened,
  pasteTooLong,
  readAttachmentText,
  TileRemove,
} from "@/components/conversation/elements/attachment-tile";
import { pastedTextLabel } from "@/components/conversation/elements/chipText";
import { composerInlineNumber } from "@/components/conversation/elements/inline-image";
import { bound, editing, rebound } from "@/components/conversation/elements/pasteBinding";
import { Button } from "@/components/controls/button";
import { Dialog, DialogContent, DialogFooter, DialogTitle } from "@/components/overlays/dialog";
import { FileDocument } from "@/components/icons";
import type { AttachmentRef } from "@/ipc/generated";
import { openLink } from "@/state/links";
import { cn } from "@/lib/utils";

/**
 * Text pasted as a chip (`[Pasted text #n]`, or a "Pasted text" card): opened in the Pages
 * editor, where it can be edited or, sent, read. A chip opens it full size (a session's tab,
 * elsewhere over the thread), its edits kept as they are made; a card in a dialog, whose Done
 * keeps them.
 */

/** Opens pasted text `n`: the composer's (`paste` null), or a sent message's. */
export type OpenPastedText = (n: number, paste: AttachmentRef | null) => void;

/** How the conversation view opens a pasted text: a session's tab, else over the thread. */
export const PastedTextOpenerContext = createContext<OpenPastedText | null>(null);

/** Whether a pasted text's bytes are Markdown (rich text pasted), not plain text. */
function isMarkdown(source: AttachmentSource): boolean {
  return (source.file?.type ?? source.ref?.mime) === "text/markdown";
}

/**
 * The Pages editor over the window with a pasted text: its Markdown, or its plain text a
 * paragraph per line. With `onDone`, edits are kept with Done; without it, it is read only.
 */
export function PastedTextDialog({
  source,
  title,
  open,
  onOpenChange,
  onDone,
}: {
  source: AttachmentSource;
  title: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDone?: ((markdown: string) => void) | undefined;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="page-paste-dialog" aria-describedby={undefined}>
        <DialogTitle>{title}</DialogTitle>
        {/* Read afresh at each opening: Done may have swapped in a new text. */}
        {open && <PastedTextEditor source={source} title={title} onDone={onDone} close={() => onOpenChange(false)} />}
      </DialogContent>
    </Dialog>
  );
}

function PastedTextEditor({
  source,
  title,
  onDone,
  onEdit,
  close,
}: {
  source: AttachmentSource;
  title: string;
  /** Done keeps the edits (a dialog's). */
  onDone?: ((markdown: string) => void) | undefined;
  /** Each edit as it is made (a full-size view's). */
  onEdit?: ((markdown: string) => void) | undefined;
  close?: () => void;
}) {
  const reader = useContext(AttachmentReaderContext);
  const [body, setBody] = useState<string | null>(null);
  // The editor's Markdown once edited, for Done.
  const edited = useRef<string | null>(null);
  const editorRef = useRef<LexicalEditor | null>(null);
  const [from] = useState(source);
  useEffect(() => {
    let live = true;
    void readAttachmentText(from, reader).then(
      (text) => live && setBody(isMarkdown(from) ? text : literalMarkdown(text)),
      () => live && setBody(""),
    );
    return () => {
      live = false;
    };
  }, [from, reader]);
  const done = () => {
    if (onDone && edited.current !== null) onDone(edited.current);
    close?.();
  };
  return (
    <>
      <div className="page-scroll" data-slot="pasted-text-editor">
        {body !== null && (
          <div className="page-column">
            <PageEditor
              body={body}
              label={title}
              editable={onDone !== undefined || onEdit !== undefined}
              editorRef={editorRef}
              onChange={(markdown) => {
                edited.current = markdown;
                onEdit?.(markdown);
              }}
              onOpenLink={(url) => void openLink(url)}
            />
          </div>
        )}
      </div>
      {onDone && (
        <DialogFooter>
          <Button size="sm" onClick={done}>
            Done
          </Button>
        </DialogFooter>
      )}
    </>
  );
}

/**
 * Pasted text `n` full size, in the Pages editor: the composer's (`paste` null), whose edits
 * go into the message as they are made, under the same number, or a sent message's, read only.
 */
export function PastedTextView({ n, paste }: { n: number; paste: AttachmentRef | null }) {
  const { attachment, source, swap } = useComposerPaste(n);
  // Read once: an edit swaps the composer's attachment, which the editor already shows. Another
  // text under the number (this one was sent or removed) is read afresh.
  const [from, setFrom] = useState(paste ? { ref: paste } : source);
  const binding = useRef(bound(attachment?.id));
  const [generation, setGeneration] = useState(0);
  const latest = useRef(swap);
  useEffect(() => {
    latest.current = swap;
  }, [swap]);
  const pending = useRef<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const id = attachment?.id;
  useEffect(() => {
    if (paste) return;
    const next = rebound(binding.current, id);
    if (next.generation !== binding.current.generation) {
      window.clearTimeout(timer.current);
      pending.current = null;
      setFrom(source);
      setGeneration(next.generation);
    }
    binding.current = next;
  }, [paste, id, source]);
  const flush = useCallback(() => {
    window.clearTimeout(timer.current);
    if (pending.current === null) return;
    binding.current = editing(binding.current);
    latest.current(pending.current);
    pending.current = null;
  }, []);
  // Closed with an edit not yet kept: it is kept now.
  useEffect(() => flush, [flush]);
  const edit = useCallback(
    (markdown: string) => {
      pending.current = markdown;
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(flush, 500);
    },
    [flush],
  );
  const label = pastedTextLabel(n);
  return (
    <div className="page-tab" data-slot="pasted-text-view">
      {from ? (
        <PastedTextEditor key={generation} source={from} title={label} onEdit={paste ? undefined : edit} />
      ) : (
        <p className="text-muted-foreground p-6 text-sm">This pasted text is no longer in the message.</p>
      )}
    </div>
  );
}

/**
 * The composer's pasted text `n`: its attachment, what it reads from, and `swap`, which puts an
 * edit in its place under the same number.
 */
function useComposerPaste(n: number) {
  const aui = useAui();
  const reader = useContext(AttachmentReaderContext);
  const attachment = useAuiState((s) =>
    s.composer.attachments.find((entry) => composerInlineNumber(entry, reader, "paste") === n),
  );
  const source: AttachmentSource | null = attachment
    ? { file: attachment.file, ref: attachment.file ? undefined : reader?.composerRef(attachment.id) }
    : null;
  const id = attachment?.id;
  const swap = useCallback(
    (markdown: string) => {
      if (!id) return;
      const composer = aui.composer();
      void composer
        .attachment({ id })
        .remove()
        .then(() => composer.addAttachment(pastedTextFile(markdown, { markdown: true, n })));
    },
    [aui, id, n],
  );
  return { attachment, source, swap };
}

/**
 * Text pasted into the message, where it was pasted: a chip with "Pasted text #n" that opens
 * it, and in the composer a × that removes it.
 */
export function InlinePastedText({
  source,
  n,
  busy = false,
  onOpen,
  onRemove,
}: {
  source: AttachmentSource | null;
  n: number;
  busy?: boolean;
  onOpen: (() => void) | null;
  onRemove?: MouseEventHandler<HTMLButtonElement> | undefined;
}) {
  const label = pastedTextLabel(n);
  return (
    <span
      data-slot="inline-paste"
      className={cn(
        "bg-muted text-foreground border-border inline-flex items-center gap-0.5 rounded-md border p-0.5 align-middle text-xs",
        source && "hover:bg-accent",
      )}
    >
      <button
        type="button"
        aria-label={`Open ${label}`}
        title={label}
        disabled={!source || !onOpen}
        onClick={() => onOpen?.()}
        className={cn("inline-flex items-center gap-1 rounded-sm px-0.5", busy && "opacity-50")}
      >
        <FileDocument aria-hidden className="size-icon-xs" />
        {label}
      </button>
      {source && pasteTooLong(source.file?.size ?? source.ref?.bytes ?? 0) && (
        <span className="text-muted-foreground pe-0.5">
          · <PasteShortened />
        </span>
      )}
      {onRemove && (
        <TileRemove
          label={`Remove ${label}`}
          onClick={onRemove}
          className="relative inset-auto size-icon-xs shrink-0 after:absolute after:-inset-1 [&>svg]:size-2"
        />
      )}
    </span>
  );
}

/** Pasted text `n` in the composer's text, its content the composer's attachment for it. */
export function ComposerPastedText({
  n,
  onRemove,
}: {
  n: number;
  onRemove: MouseEventHandler<HTMLButtonElement>;
}) {
  const open = useContext(PastedTextOpenerContext);
  const { attachment, source } = useComposerPaste(n);
  return (
    <InlinePastedText
      source={source}
      n={n}
      busy={attachment?.status.type === "running"}
      onOpen={open && (() => open(n, null))}
      onRemove={onRemove}
    />
  );
}

/** A sent message's pasted text `n`, where it was pasted: opens read only. */
export function MessagePastedText({ paste, n }: { paste: AttachmentRef | undefined; n: number }) {
  const open = useContext(PastedTextOpenerContext);
  return (
    <InlinePastedText
      source={paste ? { ref: paste } : null}
      n={n}
      onOpen={open && paste ? () => open(n, paste) : null}
    />
  );
}
