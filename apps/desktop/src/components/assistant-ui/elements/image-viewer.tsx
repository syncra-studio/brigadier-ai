import { ChevronLeft, ChevronRight, Minus, Plus, X } from "@openai/apps-sdk-ui/components/Icon";
import { Dialog as DialogPrimitive } from "radix-ui";
import { type KeyboardEvent, type MouseEvent, type ReactNode, type Ref, type WheelEvent, useImperativeHandle, useRef, useState } from "react";
import { flushSync } from "react-dom";

import { type AttachmentSource, useAttachmentUrl } from "@/components/assistant-ui/elements/attachment-url";
import { cn } from "@/lib/utils";

const ZOOM_STEP = 1.25;
const ZOOM_MIN = 0.1;
const ZOOM_MAX = 8;

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
            !morph && "data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 duration-150",
          )}
        />
        <DialogPrimitive.Content
          aria-describedby={undefined}
          data-slot="image-preview"
          onKeyDown={(event: KeyboardEvent) => {
            // Wherever focus is in the viewer (Radix puts it on a button): ‹ › step, + − 0 zoom.
            if (event.key === "ArrowLeft") step(-1);
            else if (event.key === "ArrowRight") step(1);
            else if (!zoomKeys.current?.press(event.key)) return;
            event.preventDefault();
          }}
          className={cn(
            "fixed inset-0 z-50 flex flex-col outline-none",
            !morph && "data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 duration-150",
          )}
        >
          <DialogPrimitive.Title className="sr-only">{current.name}</DialogPrimitive.Title>
          <ShownImage key={at} image={current} morph={morph && at === first} imageRef={shown} zoomKeys={zoomKeys} onBackdrop={hide} />
          <DialogPrimitive.Close
            aria-label="Close"
            title="Close"
            className="bg-secondary hover:bg-accent size-icon-button-lg rounded-capsule absolute end-4 top-4 z-10 flex items-center justify-center"
          >
            <X aria-hidden className="size-icon-md" />
          </DialogPrimitive.Close>
          {images.length > 1 && (
            <>
              <StepButton side="start" disabled={at === 0} onClick={() => step(-1)} />
              <StepButton side="end" disabled={at >= images.length - 1} onClick={() => step(1)} />
              <span data-slot="image-preview-count" className="bg-popover ring-border rounded-capsule text-foreground absolute top-5 start-1/2 -translate-x-1/2 px-3 py-1 text-sm tabular-nums ring-1 rtl:translate-x-1/2">
                {at + 1} of {images.length}
              </span>
            </>
          )}
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
        "bg-secondary hover:bg-accent size-icon-button-lg rounded-capsule absolute top-1/2 z-10 flex -translate-y-1/2 items-center justify-center disabled:opacity-30",
        side === "start" ? "start-4" : "end-4",
      )}
    >
      <Icon aria-hidden className="size-icon-md rtl:rotate-180" />
    </button>
  );
}

/** The shown image's zoom keys (+ − 0): `press` says whether it took the key. */
type ZoomKeys = { press: (key: string) => boolean };

/** The image shown, fitted to the window until zoomed, with its zoom pill. */
function ShownImage({ image, morph, imageRef, zoomKeys, onBackdrop }: {
  image: ViewerImage;
  morph: boolean;
  imageRef: { current: HTMLImageElement | null };
  zoomKeys: Ref<ZoomKeys>;
  onBackdrop: () => void;
}) {
  const resolved = useAttachmentUrl(image.source ?? {});
  const url = image.url ?? resolved;
  const frame = useRef<HTMLDivElement>(null);
  const [natural, setNatural] = useState<{ width: number; height: number; fit: number } | null>(null);
  // Shown size over the natural size; null while it fits the window.
  const [scale, setScale] = useState<number | null>(null);
  const current = scale ?? natural?.fit ?? 1;
  const zoom = (factor: number) => setScale(Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, current * factor)));
  useImperativeHandle(zoomKeys, () => ({
    press: (key) => {
      if (key === "+" || key === "=") zoom(ZOOM_STEP);
      else if (key === "-") zoom(1 / ZOOM_STEP);
      else if (key === "0") setScale(null);
      else return false;
      return true;
    },
  }));
  return (
    <>
      {/* A click beside the image closes the viewer. */}
      {/* oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions -- Esc and × close it from the keyboard */}
      <div
        ref={frame}
        data-slot="image-preview-frame"
        tabIndex={-1}
        onClick={(event) => event.target === event.currentTarget && onBackdrop()}
        onWheel={(event: WheelEvent) => {
          // A pinch reaches the page as a wheel with Ctrl held; ⌘ with the wheel zooms too.
          if (!event.ctrlKey && !event.metaKey) return;
          event.preventDefault();
          zoom(event.deltaY < 0 ? 1.1 : 1 / 1.1);
        }}
        className="flex min-h-0 flex-1 overflow-auto p-16 outline-none"
      >
        {url && (
          <img
            ref={(element) => {
              imageRef.current = element;
            }}
            src={url}
            alt={image.name}
            draggable={false}
            onLoad={(event) => {
              const element = event.currentTarget;
              const box = frame.current;
              const room = box
                ? Math.min(
                    (box.clientWidth - 2 * parseFloat(getComputedStyle(box).paddingLeft)) / element.naturalWidth,
                    (box.clientHeight - 2 * parseFloat(getComputedStyle(box).paddingTop)) / element.naturalHeight,
                  )
                : 1;
              setNatural({ width: element.naturalWidth, height: element.naturalHeight, fit: Math.min(1, room) });
            }}
            style={{
              ...(scale !== null && natural ? { width: natural.width * current, height: natural.height * current } : {}),
              ...(morph ? { viewTransitionName: MORPH } : {}),
            }}
            // Fitted by its box until zoomed, so it has its size from the first frame (a morph's end).
            className={cn("m-auto shrink-0 rounded-lg object-contain", scale === null ? "max-h-full max-w-full" : "max-w-none")}
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
          className="hover:bg-accent size-icon-button-lg rounded-capsule flex items-center justify-center disabled:opacity-50"
        >
          <Minus aria-hidden className="size-icon-md" />
        </button>
        <span aria-live="polite" className="min-w-control-lg text-center text-sm tabular-nums">{`${Math.round(current * 100)}%`}</span>
        <button
          type="button"
          aria-label="Zoom in"
          title="Zoom in"
          disabled={current >= ZOOM_MAX}
          onClick={() => zoom(ZOOM_STEP)}
          className="hover:bg-accent size-icon-button-lg rounded-capsule flex items-center justify-center disabled:opacity-50"
        >
          <Plus aria-hidden className="size-icon-md" />
        </button>
      </div>
    </>
  );
}
