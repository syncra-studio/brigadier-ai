import { ChevronLeft, ChevronRight, Download, Minus, Plus, X } from "@openai/apps-sdk-ui/components/Icon";
import { Dialog as DialogPrimitive } from "radix-ui";
import { type ComponentProps, type KeyboardEvent, type MouseEvent, type ReactNode, type Ref, useEffect, useEffectEvent, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";

import { type AttachmentSource, useAttachmentUrl } from "@/components/assistant-ui/elements/attachment-url";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu";
import { saveCopy } from "@/ipc/client";
import { toast } from "@/state/toasts";
import { cn } from "@/lib/utils";

const RAMP = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 5];
const ZOOM_MIN = 0.25;
const ZOOM_MAX = 5;
const percentage = new Intl.NumberFormat(undefined, { style: "percent", maximumFractionDigits: 0 });
// oxlint-disable-next-line brigadier/no-raw-design-values -- The image-preview specification pins this black 5% ring; no shared token exists.
const roundButton = "bg-secondary text-foreground hover:bg-popover flex h-10 min-w-10 items-center justify-center rounded-full shadow-md ring-1 ring-[rgb(0_0_0/0.05)] hover:ring-[rgb(0_0_0/0.05)] active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50";

function typing(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && !!target.closest("input, textarea, select, [contenteditable=true]");
}

async function imageBlob(image: ViewerImage, url: string | null): Promise<Blob> {
  if (image.source?.file) return image.source.file;
  if (!url) throw new Error("Image is not available");
  const response = await fetch(url);
  if (!response.ok) throw new Error("Image could not be read");
  return response.blob();
}

async function downloadImage(image: ViewerImage, url: string | null): Promise<void> {
  try {
    const blob = await imageBlob(image, url);
    const extension = ({ "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/avif": "avif", "image/svg+xml": "svg", "image/bmp": "bmp", "image/tiff": "tiff" } as Record<string, string>)[blob.type];
    const name = image.name || "image";
    await saveCopy({ name: /\.[^./\\]+$/.test(name) || !extension ? name : `${name}.${extension}`, bytes: new Uint8Array(await blob.arrayBuffer()), ask: true });
  } catch {
    toast("Could not download image", { tone: "error" });
  }
}

async function copyImage(image: ViewerImage, url: string | null): Promise<void> {
  try {
    // Start the clipboard write during the user gesture; conversion may finish asynchronously.
    const png = imageBlob(image, url).then(async (blob) => {
      if (blob.type === "image/png") return blob;
      const local = URL.createObjectURL(blob);
      try {
        const picture = new Image();
        picture.src = local;
        await picture.decode();
        const canvas = document.createElement("canvas");
        canvas.width = picture.naturalWidth;
        canvas.height = picture.naturalHeight;
        const context = canvas.getContext("2d");
        if (!context) throw new Error("Canvas unavailable");
        context.drawImage(picture, 0, 0);
        return await new Promise<Blob>((resolve, reject) => canvas.toBlob((result) => result ? resolve(result) : reject(new Error("PNG conversion failed")), "image/png"));
      } finally {
        URL.revokeObjectURL(local);
      }
    });
    await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
  } catch {
    toast("Could not copy image", { tone: "error" });
  }
}

/** Reuse around thumbnail triggers so their right-click menu matches the full image. */
export function ImageContextMenu({ image, children, ...trigger }: { image: ViewerImage; children: ReactNode } & ComponentProps<typeof ContextMenuTrigger>) {
  const resolved = useAttachmentUrl(image.source ?? {});
  const url = image.url ?? resolved;
  // What a trigger around it passes on (the viewer's click, its ref) reaches the thumbnail too.
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild {...trigger}>
        {children}
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onSelect={() => void copyImage(image, url)}>Copy image</ContextMenuItem>
        <ContextMenuItem onSelect={() => void downloadImage(image, url)}>Download image</ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/** The name the thumbnail and the shown image share while one morphs into the other. */
const MORPH = "image-viewer-image";

/** One image a viewer can show: its URL, or the attachment it is read from while shown. */
export type ViewerImage = { name: string; url?: string | null; source?: AttachmentSource };

type ViewTransition = { finished: Promise<void> };
type WithTransitions = Document & { startViewTransition?: (update: () => Promise<void> | void) => ViewTransition };

/** Whether to morph: the browser can, and the user hasn't asked for less motion. */
function morphs(): boolean {
  return typeof (document as WithTransitions).startViewTransition === "function" && !matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * The full-window image viewer: the image over a near-black backdrop, grown out of the thumbnail
 * that opened it and shrunk back into it on close (a plain fade where the browser can't morph,
 * at once with less motion); "− 100% +" zoom (also +, −, 0, and pinch or ⌘-wheel); ‹ › and ←/→
 * between the images of a set, with "2 of 5"; Esc, × or a click on the backdrop closes it.
 * `children` is the thumbnail that opens it.
 */
export function ImagePreview({
  url,
  name,
  gallery,
  children,
}: {
  url: string | null;
  name: string;
  /** The set this image belongs to (a message's images), and where it is in it. */
  gallery?: { images: readonly ViewerImage[]; index: number } | undefined;
  children: ReactNode;
}) {
  const images = gallery?.images ?? [{ name, url }];
  const first = gallery?.index ?? 0;
  const [open, setOpen] = useState(false);
  const [at, setAt] = useState(first);
  const [morph, setMorph] = useState(false);
  const thumbnail = useRef<HTMLElement | null>(null);
  const shown = useRef<HTMLImageElement | null>(null);

  const show = (opener: HTMLElement) => {
    thumbnail.current = opener.querySelector("img") ?? opener;
    setAt(first);
    if (!morphs()) {
      setOpen(true);
      return;
    }
    const thumb = thumbnail.current;
    thumb.style.viewTransitionName = MORPH;
    (document as WithTransitions).startViewTransition!(async () => {
      thumb.style.viewTransitionName = "";
      flushSync(() => {
        setMorph(true);
        setOpen(true);
      });
      // The new picture is taken once the image can be drawn.
      await shown.current?.decode().catch(() => undefined);
    }).finished.finally(() => setMorph(false));
  };

  const hide = () => {
    const thumb = thumbnail.current;
    // Back into its thumbnail only from the image that opened it.
    if (!morphs() || !thumb || at !== first) {
      setOpen(false);
      return;
    }
    // The shown image takes the shared name first, so the morph starts from it.
    flushSync(() => setMorph(true));
    (document as WithTransitions).startViewTransition!(() => {
      flushSync(() => setOpen(false));
      thumb.style.viewTransitionName = MORPH;
    }).finished.finally(() => {
      thumb.style.viewTransitionName = "";
      setMorph(false);
    });
  };

  const zoomKeys = useRef<ZoomKeys>(null);
  const step = (by: number) => setAt((current) => Math.min(images.length - 1, Math.max(0, current + by)));
  const current = images[at] ?? { name, url };
  return (
    <DialogPrimitive.Root open={open} onOpenChange={(next) => (next ? undefined : hide())}>
      <DialogPrimitive.Trigger
        asChild
        onClick={(event: MouseEvent<HTMLElement>) => {
          event.preventDefault();
          show(event.currentTarget);
        }}
      >
        {children}
      </DialogPrimitive.Trigger>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay
          className={cn(
            "bg-image-viewer fixed inset-0 z-50",
            !morph && "data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 duration-150 motion-reduce:animate-none motion-reduce:duration-0",
          )}
        />
        <DialogPrimitive.Content
          aria-describedby={undefined}
          data-slot="image-preview"
          onKeyDown={(event: KeyboardEvent) => {
            if (typing(event.target) || event.altKey || event.ctrlKey || event.metaKey) return;
            if (event.shiftKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) return;
            if (event.key === "ArrowLeft") step(-1);
            else if (event.key === "ArrowRight") step(1);
            else if (!zoomKeys.current?.press(event.key)) return;
            event.preventDefault();
          }}
          onEscapeKeyDown={(event) => { if (typing(event.target)) event.preventDefault(); }}
          onClick={(event) => { if (event.target === event.currentTarget) hide(); }}
          className={cn(
            "bg-image-viewer fixed inset-0 z-50 flex flex-col items-center justify-center px-16 py-16 outline-none",
            !morph && "data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 duration-150 motion-reduce:animate-none motion-reduce:duration-0",
          )}
        >
          <DialogPrimitive.Title className="sr-only">Image preview</DialogPrimitive.Title>
          <ShownImage key={at} image={current} morph={morph && at === first} imageRef={shown} zoomKeys={zoomKeys} onBackdrop={hide} count={images.length > 1 ? `${at + 1} of ${images.length}` : null} />
          {images.length > 1 && <>
            <StepButton side="start" disabled={at === 0} onClick={() => step(-1)} />
            <StepButton side="end" disabled={at >= images.length - 1} onClick={() => step(1)} />
          </>}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

/** ‹ or › at mid-height, to the image before or after. */
function StepButton({ side, disabled, onClick }: { side: "start" | "end"; disabled: boolean; onClick: () => void }) {
  const Icon = side === "start" ? ChevronLeft : ChevronRight;
  return (
    <button
      type="button"
      aria-label={side === "start" ? "Previous image" : "Next image"}
      title={side === "start" ? "Previous image" : "Next image"}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        roundButton, "absolute top-1/2 z-10 -translate-y-1/2",
        side === "start" ? "left-3" : "right-3",
      )}
    >
      <Icon aria-hidden className="size-icon-md rtl:rotate-180" />
    </button>
  );
}

/** The shown image's zoom keys (+ − 0): `press` says whether it took the key. */
type ZoomKeys = { press: (key: string) => boolean };

/** The image is centred when it fits; its overflow is panned by dragging. */
function ShownImage({ image, morph, imageRef, zoomKeys, onBackdrop, count }: {
  image: ViewerImage;
  morph: boolean;
  imageRef: { current: HTMLImageElement | null };
  zoomKeys: Ref<ZoomKeys>;
  onBackdrop: () => void;
  count: string | null;
}) {
  const resolved = useAttachmentUrl(image.source ?? {});
  const url = image.url ?? resolved;
  const frame = useRef<HTMLDivElement>(null);
  const [natural, setNatural] = useState<{ width: number; height: number } | null>(null);
  const [room, setRoom] = useState({ width: innerWidth - 128, height: innerHeight - 240 });
  const [scale, setScale] = useState<number | null>(null);
  const fit = natural ? Math.min(1, room.width / natural.width, room.height / natural.height) : 1;
  const current = scale ?? fit;
  const anchor = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const drag = useRef<{ id: number; x: number; y: number; left: number; top: number; moved: boolean } | null>(null);
  const suppressClick = useRef(false);
  const [panning, setPanning] = useState(false);
  const viewWidth = natural ? natural.width * fit : room.width;
  const viewHeight = natural ? natural.height * fit : room.height;
  const canPan = !!natural && current > fit;
  useEffect(() => {
    const resize = () => setRoom({ width: Math.max(1, innerWidth - 128), height: Math.max(1, innerHeight - 240) });
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  const zoomTo = (next: number, x?: number, y?: number) => {
    const box = frame.current;
    if (!box || !natural) return;
    next = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, next));
    const bounds = box.getBoundingClientRect();
    const px = x === undefined ? box.clientWidth / 2 : x - bounds.left;
    const py = y === undefined ? box.clientHeight / 2 : y - bounds.top;
    anchor.current = {
      x: (box.scrollLeft + px - Math.max(0, (box.clientWidth - natural.width * current) / 2)) / current,
      y: (box.scrollTop + py - Math.max(0, (box.clientHeight - natural.height * current) / 2)) / current,
      left: px, top: py,
    };
    setScale(next);
  };
  useLayoutEffect(() => {
    const box = frame.current;
    const point = anchor.current;
    if (!box || !natural || !point) return;
    box.scrollLeft = point.x * current + Math.max(0, (box.clientWidth - natural.width * current) / 2) - point.left;
    box.scrollTop = point.y * current + Math.max(0, (box.clientHeight - natural.height * current) / 2) - point.top;
    anchor.current = null;
  }, [current, natural]);
  const stepZoom = (by: number) => {
    const ramp = [...RAMP, Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, fit))].toSorted((a, b) => a - b);
    zoomTo(by > 0 ? ramp.find((value) => value > current + 0.00001) ?? ZOOM_MAX : ramp.findLast((value) => value < current - 0.00001) ?? ZOOM_MIN);
  };
  useImperativeHandle(zoomKeys, () => ({ press: (key) => {
    if (key === "+" || key === "=") stepZoom(1);
    else if (key === "-") stepZoom(-1);
    else if (key === "0") { anchor.current = null; setScale(null); if (frame.current) { frame.current.scrollLeft = 0; frame.current.scrollTop = 0; } }
    else return false;
    return true;
  } }));
  // A native non-passive listener also suppresses the webview's own pinch/page zoom.
  const wheelZoom = useEffectEvent((event: globalThis.WheelEvent) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 800 : 1);
    zoomTo(current * Math.exp(-delta / 200), event.clientX, event.clientY);
  });
  useEffect(() => {
    const box = frame.current;
    const wheel = (event: globalThis.WheelEvent) => wheelZoom(event);
    box?.addEventListener("wheel", wheel, { passive: false });
    return () => box?.removeEventListener("wheel", wheel);
  }, []);
  return <>
    <div className="absolute top-3 right-3 z-10 flex gap-2">
      <button type="button" aria-label="Download image" title="Download image" className={roundButton} disabled={!url} onClick={() => { void downloadImage(image, url); }}><Download aria-hidden className="size-icon-md" /></button>
      <DialogPrimitive.Close aria-label="Close image preview" title="Close image preview" className={roundButton}><X aria-hidden className="size-icon-md" /></DialogPrimitive.Close>
    </div>
    {/* oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions -- Esc and the close button provide keyboard dismissal */}
    <div ref={frame} data-slot="image-preview-frame" tabIndex={-1}
      className="min-h-0 shrink-0 overflow-hidden outline-none select-none"
      style={{ width: viewWidth, height: viewHeight, cursor: canPan ? panning ? "grabbing" : "grab" : undefined, touchAction: "none" }}
      onClick={(event) => { if (suppressClick.current) { suppressClick.current = false; return; } if (event.target === event.currentTarget || (event.target instanceof HTMLElement && event.target.dataset.slot === "image-preview-canvas")) onBackdrop(); }}
      onPointerDown={(event) => {
        if (!canPan || event.button !== 0) return;
        const box = event.currentTarget;
        box.setPointerCapture(event.pointerId);
        drag.current = { id: event.pointerId, x: event.clientX, y: event.clientY, left: box.scrollLeft, top: box.scrollTop, moved: false };
        setPanning(true);
      }}
      onPointerMove={(event) => {
        const start = drag.current;
        if (!start || start.id !== event.pointerId) return;
        start.moved ||= Math.hypot(event.clientX - start.x, event.clientY - start.y) > 3;
        event.currentTarget.scrollLeft = start.left - (event.clientX - start.x);
        event.currentTarget.scrollTop = start.top - (event.clientY - start.y);
      }}
      onPointerUp={(event) => {
        if (!drag.current || drag.current.id !== event.pointerId) return;
        suppressClick.current = drag.current.moved;
        drag.current = null;
        setPanning(false);
        event.currentTarget.releasePointerCapture(event.pointerId);
      }}
      onLostPointerCapture={() => { drag.current = null; setPanning(false); }}
    >
      <div data-slot="image-preview-canvas" className="flex min-h-full min-w-full items-center justify-center" style={natural ? { width: natural.width * current, height: natural.height * current } : undefined}>
        {url && <ImageContextMenu image={{ ...image, url }}><img
          ref={(element) => { imageRef.current = element; }} src={url} alt={image.name} draggable={false}
          onLoad={(event) => setNatural({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })}
          style={{ ...(natural ? { width: natural.width * current, height: natural.height * current } : { maxWidth: "100%", maxHeight: room.height }), ...(morph ? { viewTransitionName: MORPH } : {}) }}
          className="shrink-0 rounded-lg object-contain max-w-none"
        /></ImageContextMenu>}
      </div>
    </div>
    {/* oxlint-disable-next-line brigadier/no-raw-design-values -- The task pins this responsive maximum width. */}
    <div data-slot="image-preview-controls" className="mt-5 flex max-w-[min(48rem,calc(100vw-2rem))] flex-col items-center gap-3 text-foreground">
      <div data-slot="image-preview-zoom" className="flex items-center gap-1 rounded-full bg-foreground/10 p-1">
        <button type="button" aria-label="Zoom out image" title="Zoom out image" disabled={current <= ZOOM_MIN} onClick={() => stepZoom(-1)} className="flex size-9 items-center justify-center rounded-full bg-foreground/10 hover:bg-foreground/20 active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"><Minus aria-hidden className="size-icon-md" /></button>
        <span data-slot="image-preview-scale" aria-live="polite" className="min-w-16 text-center text-sm tabular-nums">{percentage.format(current)}</span>
        <button type="button" aria-label="Zoom in image" title="Zoom in image" disabled={current >= ZOOM_MAX} onClick={() => stepZoom(1)} className="flex size-9 items-center justify-center rounded-full bg-foreground/10 hover:bg-foreground/20 active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"><Plus aria-hidden className="size-icon-md" /></button>
      </div>
      {count && <span data-slot="image-preview-count" className="text-sm tabular-nums">{count}</span>}
    </div>
  </>;
}
