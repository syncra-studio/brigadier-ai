import { useAui, useAuiState } from "@assistant-ui/react";
import { FileDocument } from "@openai/apps-sdk-ui/components/Icon";
import type { LexicalEditor } from "lexical";
import { type MouseEventHandler, useContext, useEffect, useRef, useState } from "react";

import { literalMarkdown } from "@/app/conversation/composerMarkdown";
import { PageEditor } from "@/app/conversation/pages/PageEditor";
import "@/app/conversation/pages/PageTab.css";
import {
  AttachmentReaderContext,
  type AttachmentSource,
  pastedTextFile,
  PasteShortened,
  pasteTooLong,
  readAttachmentText,
  TileRemove,
} from "@/components/assistant-ui/elements/attachment-tile";
import { pastedTextLabel } from "@/components/assistant-ui/elements/chipText";
import { composerInlineNumber } from "@/components/assistant-ui/elements/inline-image";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogTitle } from "@/components/ui/dialog";
import type { AttachmentRef } from "@/ipc/generated";
import { openLink } from "@/state/links";
import { cn } from "@/lib/utils";

/**
 * Text pasted as a chip (`[Pasted text #n]`, or a "Pasted text" card): opened in the Pages
 * editor, where it can be edited (Done keeps the edits) or, sent, read.
 */

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
  close,
}: {
  source: AttachmentSource;
  title: string;
  onDone?: ((markdown: string) => void) | undefined;
  close: () => void;
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
    close();
  };
  return (
    <>
      <div className="page-scroll" data-slot="pasted-text-editor">
        {body !== null && (
          <PageEditor
            body={body}
            label={title}
            editable={onDone !== undefined}
            editorRef={editorRef}
            onChange={(markdown) => {
              edited.current = markdown;
            }}
            onOpenLink={(url) => void openLink(url)}
          />
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
 * Text pasted into the message, where it was pasted: a chip with "Pasted text #n" that opens
 * it, and in the composer a × that removes it.
 */
export function InlinePastedText({
  source,
  n,
  busy = false,
  onRemove,
  onDone,
}: {
  source: AttachmentSource | null;
  n: number;
  busy?: boolean;
  onRemove?: MouseEventHandler<HTMLButtonElement> | undefined;
  onDone?: ((markdown: string) => void) | undefined;
}) {
  const [open, setOpen] = useState(false);
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
        disabled={!source}
        onClick={() => setOpen(true)}
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
      {source && (
        <PastedTextDialog source={source} title={label} open={open} onOpenChange={setOpen} onDone={onDone} />
      )}
    </span>
  );
}

/**
 * Pasted text `n` in the composer's text, its content the composer's attachment for it; Done
 * in the editor swaps in the edited text, under the same number.
 */
export function ComposerPastedText({
  n,
  onRemove,
}: {
  n: number;
  onRemove: MouseEventHandler<HTMLButtonElement>;
}) {
  const aui = useAui();
  const reader = useContext(AttachmentReaderContext);
  const attachment = useAuiState((s) =>
    s.composer.attachments.find((entry) => composerInlineNumber(entry, reader, "paste") === n),
  );
  const source = attachment
    ? { file: attachment.file, ref: attachment.file ? undefined : reader?.composerRef(attachment.id) }
    : null;
  const done = (markdown: string) => {
    if (!attachment) return;
    const composer = aui.composer();
    void composer
      .attachment({ id: attachment.id })
      .remove()
      .then(() => composer.addAttachment(pastedTextFile(markdown, { markdown: true, n })));
  };
  return (
    <InlinePastedText
      source={source}
      n={n}
      busy={attachment?.status.type === "running"}
      onRemove={onRemove}
      onDone={done}
    />
  );
}

/** A sent message's pasted text `n`, where it was pasted: opens read only. */
export function MessagePastedText({ paste, n }: { paste: AttachmentRef | undefined; n: number }) {
  return <InlinePastedText source={paste ? { ref: paste } : null} n={n} />;
}
