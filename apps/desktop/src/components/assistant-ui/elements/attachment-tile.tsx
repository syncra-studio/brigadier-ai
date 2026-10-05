import {
  FileDocument,
  FileImage,
  Minus,
  Paperclip,
  Plus,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { Dialog as DialogPrimitive } from "radix-ui";
import {
  type ComponentPropsWithRef,
  createContext,
  type KeyboardEvent,
  type ReactNode,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";

import type { AttachmentRef } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/**
 * Where attachment bytes come from, provided by the app: the daemon's blob store, and the
 * stored reference behind a composer attachment that has no local file (one pulled back from
 * the queue).
 */
export type AttachmentReader = {
  read: (ref: AttachmentRef) => Promise<Blob>;
  composerRef: (attachmentId: string) => AttachmentRef | undefined;
};

export const AttachmentReaderContext = createContext<AttachmentReader | null>(null);

/** A tile's bytes: a file not stored yet, or a stored reference. */
export type AttachmentSource = { file?: File | undefined; ref?: AttachmentRef | undefined };

/** A paste this long becomes a "Pasted text" attachment instead of text. */
export const PASTE_AS_ATTACHMENT_CHARS = 5000;
/**
 * Pasted text up to this size goes to the model whole, as part of the message; longer text
 * goes as its start and end (INLINE_TEXT_MAX_BYTES in the core).
 */
export const PASTE_INLINE_BYTES = 200_000;
const PASTED_TEXT_NAME = "Pasted text.txt";

/** The files the composer made from long pastes, as opposed to files the user attached. */
const pastedFiles = new WeakSet<File>();

/** A long paste as a file to attach; it is sent as text the user pasted. */
export function pastedTextFile(text: string): File {
  const file = new File([text], PASTED_TEXT_NAME, { type: "text/plain" });
  pastedFiles.add(file);
  return file;
}

export function isPastedFile(file: File): boolean {
  return pastedFiles.has(file);
}

/** Images pasted into the text, by their `[Image #n]` number, as opposed to attached ones. */
const inlineFiles = new WeakMap<File, number>();

/** An image pasted into the text as `[Image #n]`, to attach as that image. */
export function inlineImageFile(file: File, n: number): File {
  inlineFiles.set(file, n);
  return file;
}

/** The `[Image #n]` number of an image pasted into the text, if `file` is one. */
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

/** Object URLs of stored attachments by content hash, least recently used first. */
const storedUrls = new Map<string, Promise<string>>();
const STORED_URLS = 32;

function storedUrl(ref: AttachmentRef, read: AttachmentReader["read"]): Promise<string> {
  const cached = storedUrls.get(ref.id);
  if (cached) {
    storedUrls.delete(ref.id);
    storedUrls.set(ref.id, cached);
    return cached;
  }
  const url = read(ref).then((blob) => URL.createObjectURL(blob));
  url.catch(() => storedUrls.delete(ref.id));
  storedUrls.set(ref.id, url);
  const oldest = storedUrls.keys().next().value;
  if (storedUrls.size > STORED_URLS && oldest !== undefined) {
    void storedUrls.get(oldest)?.then(URL.revokeObjectURL, () => undefined);
    storedUrls.delete(oldest);
  }
  return url;
}

/** An object URL for the attachment's bytes, once there is one. */
export function useAttachmentUrl({ file, ref }: AttachmentSource): string | null {
  const reader = useContext(AttachmentReaderContext);
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    // A local file's URL is this tile's own; a stored one is shared through the cache.
    const local = file ? URL.createObjectURL(file) : null;
    const next = local ? Promise.resolve(local) : ref && reader ? storedUrl(ref, reader.read) : null;
    if (!next) return undefined;
    let live = true;
    next.then(
      (loaded) => live && setUrl(loaded),
      () => live && setUrl(null),
    );
    return () => {
      live = false;
      if (local) URL.revokeObjectURL(local);
    };
  }, [file, ref, reader]);
  return url;
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
        "bg-foreground text-background focus-visible:ring-ring absolute end-1.5 top-1.5 z-10 flex size-icon-md items-center justify-center rounded-capsule outline-none focus-visible:ring-2",
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
  children,
}: {
  source: AttachmentSource;
  name: string;
  /** Still being stored: dimmed, with a spinner passed in `children`. */
  busy?: boolean;
  children?: ReactNode;
}) {
  const url = useAttachmentUrl(source);
  return (
    <div data-slot="attachment-image" className="group/tile size-attachment-thumb relative shrink-0">
      <ImagePreview url={url} name={name}>
        <button
          type="button"
          aria-label={`Preview ${name}`}
          title={name}
          disabled={!url}
          className="bg-muted focus-visible:ring-ring size-full overflow-hidden rounded-2xl outline-none focus-visible:ring-2"
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

const ZOOM_STEP = 1.25;
const ZOOM_MIN = 0.1;
const ZOOM_MAX = 8;

/**
 * The "Image preview": the image over the dimmed window, fitted to it, with a close
 * button at the top right and a "− 100% +" zoom pill at the bottom (+, − and 0 zoom too).
 */
export function ImagePreview({
  url,
  name,
  children,
}: {
  url: string | null;
  name: string;
  children: ReactNode;
}) {
  const frame = useRef<HTMLDivElement>(null);
  const [natural, setNatural] = useState<{ width: number; height: number; fit: number } | null>(
    null,
  );
  // Shown size over the natural size; null until the image is measured (then it fits).
  const [scale, setScale] = useState<number | null>(null);
  const current = scale ?? natural?.fit ?? 1;
  const zoom = (factor: number) =>
    setScale(Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, current * factor)));
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "+" || event.key === "=") zoom(ZOOM_STEP);
    else if (event.key === "-") zoom(1 / ZOOM_STEP);
    else if (event.key === "0") setScale(null);
    else return;
    event.preventDefault();
  };
  return (
    <DialogPrimitive.Root onOpenChange={(open) => open && setScale(null)}>
      <DialogPrimitive.Trigger asChild>{children}</DialogPrimitive.Trigger>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="bg-background/85 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 fixed inset-0 z-50 duration-150" />
        <DialogPrimitive.Content
          aria-describedby={undefined}
          onKeyDown={onKeyDown}
          data-slot="image-preview"
          className="data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 fixed inset-0 z-50 flex flex-col outline-none duration-150"
        >
          <DialogPrimitive.Title className="sr-only">{name}</DialogPrimitive.Title>
          <DialogPrimitive.Close
            aria-label="Close"
            title="Close"
            className="bg-secondary hover:bg-accent focus-visible:ring-ring size-icon-button-lg rounded-capsule absolute end-4 top-4 z-10 flex items-center justify-center outline-none focus-visible:ring-2"
          >
            <X aria-hidden className="size-icon-md" />
          </DialogPrimitive.Close>
          <div ref={frame} className="flex min-h-0 flex-1 overflow-auto p-16">
            {url && (
              <img
                src={url}
                alt={name}
                draggable={false}
                onLoad={(event) => {
                  const image = event.currentTarget;
                  const box = frame.current;
                  const room = box
                    ? Math.min(
                        (box.clientWidth - 2 * parseFloat(getComputedStyle(box).paddingLeft)) /
                          image.naturalWidth,
                        (box.clientHeight - 2 * parseFloat(getComputedStyle(box).paddingTop)) /
                          image.naturalHeight,
                      )
                    : 1;
                  setNatural({
                    width: image.naturalWidth,
                    height: image.naturalHeight,
                    fit: Math.min(1, room),
                  });
                }}
                style={
                  natural
                    ? { width: natural.width * current, height: natural.height * current }
                    : undefined
                }
                className={cn(
                  "m-auto max-w-none shrink-0 rounded-lg object-contain",
                  !natural && "invisible",
                )}
              />
            )}
          </div>
          <div className="bg-popover ring-border rounded-capsule absolute bottom-6 start-1/2 flex -translate-x-1/2 items-center gap-1 p-1 ring-1 rtl:translate-x-1/2">
            <button
              type="button"
              aria-label="Zoom out"
              title="Zoom out"
              disabled={current <= ZOOM_MIN}
              onClick={() => zoom(1 / ZOOM_STEP)}
              className="hover:bg-accent focus-visible:ring-ring size-icon-button-lg rounded-capsule flex items-center justify-center outline-none focus-visible:ring-2 disabled:opacity-50"
            >
              <Minus aria-hidden className="size-icon-md" />
            </button>
            <span
              aria-live="polite"
              className="min-w-control-lg text-center text-sm tabular-nums"
            >{`${Math.round(current * 100)}%`}</span>
            <button
              type="button"
              aria-label="Zoom in"
              title="Zoom in"
              disabled={current >= ZOOM_MAX}
              onClick={() => zoom(ZOOM_STEP)}
              className="hover:bg-accent focus-visible:ring-ring size-icon-button-lg rounded-capsule flex items-center justify-center outline-none focus-visible:ring-2 disabled:opacity-50"
            >
              <Plus aria-hidden className="size-icon-md" />
            </button>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
