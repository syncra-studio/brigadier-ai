import {
  FileDocument,
  FileImage,
  Paperclip,
  Warning,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  type ComponentPropsWithRef,
  type ReactNode,
  useContext,
  useEffect,
  useState,
} from "react";

import {
  type AttachmentReader,
  AttachmentReaderContext,
  type AttachmentSource,
  useAttachment,
  useAttachmentUrl,
} from "@/components/assistant-ui/elements/attachment-url";
import { ImageContextMenu, ImagePreview, type ViewerImage } from "@/components/assistant-ui/elements/image-viewer";

export { type AttachmentReader, AttachmentReaderContext, type AttachmentSource, useAttachmentUrl };
export { ImagePreview };
import { cn } from "@/lib/utils";

/**
 * Pasted text up to this size goes to the model whole, as part of the message; longer text
 * goes as its start and end (INLINE_TEXT_MAX_BYTES in the core).
 */
export const PASTE_INLINE_BYTES = 200_000;

/** The files the composer made from long pastes, as opposed to files the user attached. */
const pastedFiles = new WeakSet<File>();

/**
 * A paste as a file to attach; it is sent as text the user pasted. `markdown`: rich text
 * pasted, as Markdown. With `n`, it is `[Pasted text #n]` in the text, where it was pasted.
 */
export function pastedTextFile(text: string, { markdown = false, n = null }: { markdown?: boolean; n?: number | null } = {}): File {
  const file = markdown
    ? new File([text], "Pasted text.md", { type: "text/markdown" })
    : new File([text], "Pasted text.txt", { type: "text/plain" });
  pastedFiles.add(file);
  if (n !== null) inlineFiles.set(file, n);
  return file;
}

export function isPastedFile(file: File): boolean {
  return pastedFiles.has(file);
}

/**
 * Images and texts pasted into the text, by their `[Image #n]` or `[Pasted text #n]` number, as
 * opposed to attached ones.
 */
const inlineFiles = new WeakMap<File, number>();

/** An image pasted into the text as `[Image #n]`, to attach as that image. */
export function inlineImageFile(file: File, n: number): File {
  inlineFiles.set(file, n);
  return file;
}

/** The number of an image or a text pasted into the text, if `file` is one. */
export function inlineNumberOf(file: File): number | null {
  return inlineFiles.get(file) ?? null;
}

/** Whether pasted text is too long to go to the model whole. */
export function pasteTooLong(bytes: number): boolean {
  return bytes > PASTE_INLINE_BYTES;
}

/** What a pasted text's card says when it is too long to go to the model whole. */
export function PasteShortened() {
  return (
    <span
      title={`Over ${PASTE_INLINE_BYTES / 1000} kB, so the model gets its start and end. In a session, workers can get all of it.`}
    >
      Start and end are sent
    </span>
  );
}

/** "PDF", "Text", "PNG image": what a file card says under its name. */
export function kindOf(name: string, mime: string): string {
  if (mime === "application/pdf") return "PDF";
  const extension = name.includes(".") ? name.split(".").pop()?.toUpperCase() : undefined;
  if (mime.startsWith("image/")) return extension ? `${extension} image` : "Image";
  if (mime.startsWith("text/")) return extension ? `${extension} text` : "Text";
  return extension ? `${extension} file` : "File";
}

export function AttachmentIcon({ mime, className }: { mime: string; className?: string }) {
  const Icon = mime.startsWith("image/")
    ? FileImage
    : mime.startsWith("text/") || mime === "application/pdf"
      ? FileDocument
      : Paperclip;
  return <Icon aria-hidden className={cn("size-icon-sm shrink-0", className)} />;
}

/** The attachment's text (a pasted text), read once. */
export function useAttachmentText({ file, ref }: AttachmentSource): string | null {
  const reader = useContext(AttachmentReaderContext);
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    const blob = file ? Promise.resolve(file) : ref && reader ? reader.read(ref) : null;
    if (!blob) return undefined;
    let live = true;
    blob
      .then((bytes) => bytes.text())
      .then(
        (next) => live && setText(next),
        () => live && setText(null),
      );
    return () => {
      live = false;
    };
  }, [file, ref, reader]);
  return text;
}

/** Reads an attachment's text now (for "Show in text field"). */
export function readAttachmentText(
  { file, ref }: AttachmentSource,
  reader: AttachmentReader | null,
): Promise<string> {
  if (file) return file.text();
  if (ref && reader) return reader.read(ref).then((blob) => blob.text());
  return Promise.reject(new Error("The attachment's text is not available."));
}

/** The × at a tile's top-right corner; on images it shows on hover or focus. */
export function TileRemove({
  label,
  onHover = false,
  className,
  ...props
}: ComponentPropsWithRef<"button"> & { label: string; onHover?: boolean }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      {...props}
      className={cn(
        "bg-foreground text-background absolute end-1.5 top-1.5 z-10 flex size-icon-md items-center justify-center rounded-capsule",
        onHover &&
          "opacity-0 transition-opacity group-hover/tile:opacity-100 focus-visible:opacity-100",
        className,
      )}
    >
      <X aria-hidden className="size-icon-xs" />
    </button>
  );
}

/**
 * The image attachment: a rounded square thumbnail that opens a full-window preview.
 * `children` go over it (the remove button).
 */
export function ImageTile({
  source,
  name,
  busy = false,
  sent = false,
  gallery,
  children,
}: {
  source: AttachmentSource;
  name: string;
  /** Still being stored: dimmed, with a spinner passed in `children`. */
  busy?: boolean;
  /** In a sent message: a smaller tile above the bubble. */
  sent?: boolean;
  /** The images it is shown among, to step between in the viewer. */
  gallery?: { images: readonly ViewerImage[]; index: number } | undefined;
  children?: ReactNode;
}) {
  const { url, failed } = useAttachment(source);
  // A sent image that can't be read: a box that says so, in place of an empty thumbnail.
  if (sent && failed) {
    return (
      <div
        data-slot="attachment-image"
        data-state="failed"
        title={name}
        className="bg-muted text-muted-foreground size-attachment-sent flex shrink-0 flex-col items-center justify-center gap-1 rounded-2xl text-xs"
      >
        <Warning aria-hidden className="size-icon-sm" />
        Failed
      </div>
    );
  }
  return (
    <div
      data-slot="attachment-image"
      data-state={url ? undefined : "loading"}
      className={cn("group/tile relative shrink-0", sent ? "size-attachment-sent" : "size-attachment-thumb")}
    >
      <ImagePreview url={url} name={name} gallery={gallery}>
        <ImageContextMenu image={{ name, url, source }}>
        <button
          type="button"
          aria-label={`Preview ${name}`}
          title={name}
          disabled={!url}
          // Loading, the box pulses.
          className={cn("bg-muted size-full overflow-hidden rounded-2xl", !url && "animate-pulse motion-reduce:animate-none")}
        >
          {url && (
            <img
              src={url}
              alt={name}
              draggable={false}
              className={cn("size-full object-cover", busy && "opacity-50")}
            />
          )}
        </button>
        </ImageContextMenu>
      </ImagePreview>
      <span
        aria-hidden
        className="ring-foreground/10 pointer-events-none absolute inset-0 rounded-2xl ring-1 ring-inset"
      />
      {children}
    </div>
  );
}

/** A file attachment: an icon tile, the name, and a second line (its kind, or an action). */
export function FileTile({
  name,
  mime,
  title,
  detail,
  icon,
  className,
  children,
}: {
  name: string;
  mime: string;
  /** The first line; the file name by default. */
  title?: ReactNode;
  detail: ReactNode;
  /** Replaces the kind icon (e.g. a spinner while storing). */
  icon?: ReactNode;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <div
      data-slot="attachment-file"
      title={name}
      className={cn(
        "group/tile border-border w-attachment-card relative flex shrink-0 items-center gap-2 rounded-xl border p-1.5 pe-7",
        className,
      )}
    >
      <span className="bg-background text-muted-foreground size-control-lg flex shrink-0 items-center justify-center rounded-lg">
        {icon ?? <AttachmentIcon mime={mime} className="size-icon-md" />}
      </span>
      <span className="flex min-w-0 flex-col text-start">
        <span className="truncate text-sm">{title ?? name}</span>
        <span className="text-muted-foreground truncate text-xs">{detail}</span>
      </span>
      {children}
    </div>
  );
}

