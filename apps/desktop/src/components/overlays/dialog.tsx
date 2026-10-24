import type * as React from "react";
import { Dialog as DialogPrimitive } from "radix-ui";

import { X } from "@/components/icons";
import { cn } from "@/lib/utils";
import { Button } from "@/components/controls/button";

function Dialog({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
}

function DialogTrigger({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />;
}

function DialogPortal({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />;
}

function DialogClose({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />;
}

function DialogOverlay({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        "data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=open]:animate-in bg-background/60 fixed inset-0 isolate z-50 duration-100 supports-backdrop-filter:backdrop-blur-xs",
        className,
      )}
      {...props}
    />
  );
}

/** An overlay with no look of its own, for a dialog that draws its own (the image viewer). */
function DialogPlainOverlay(props: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  return <DialogPrimitive.Overlay {...props} />;
}

/** Dialog content with no look and no close button of its own. */
function DialogPlainContent(props: React.ComponentProps<typeof DialogPrimitive.Content>) {
  return <DialogPrimitive.Content {...props} />;
}

/**
 * Keeps an open dialog's top edge where it first opened, centred in the window. A centred card
 * whose content changes after it opens (a preview that arrives, a line that loads) moves both
 * edges at once, a jump; held by its top it only grows or shrinks downward. It moves up only as
 * far as it must to stay in the window, and centres again when the window resizes. A dialog that
 * places its top itself (a `top-*` class, which replaces `top-(--dialog-top)`) keeps its own.
 */
function pinTop(card: HTMLDivElement | null) {
  if (!card) return;
  const room = () => document.documentElement.clientHeight - card.offsetHeight;
  let top = 0;
  const place = () => {
    card.style.setProperty("--dialog-top", `${Math.max(0, Math.min(top, room()))}px`);
  };
  const centre = () => {
    top = Math.round(Math.max(0, room() / 2));
    place();
  };
  centre();
  const observer = new ResizeObserver(place);
  observer.observe(card);
  window.addEventListener("resize", centre);
  return () => {
    observer.disconnect();
    window.removeEventListener("resize", centre);
  };
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  ...props
}: Omit<React.ComponentProps<typeof DialogPrimitive.Content>, "ref"> & {
  showCloseButton?: boolean;
}) {
  return (
    <DialogPortal data-slot="dialog-portal">
      <DialogOverlay />
      <DialogPrimitive.Content
        ref={pinTop}
        data-slot="dialog-content"
        className={cn(
          "data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 bg-popover text-popover-foreground ring-foreground/10 data-[state=closed]:animate-out data-[state=open]:animate-in rounded-dialog fixed start-1/2 top-(--dialog-top) z-50 grid w-full max-w-sm -translate-x-1/2 gap-4 p-5 text-sm ring-1 duration-100 outline-none rtl:translate-x-1/2",
          className,
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close
            data-slot="dialog-close"
            className="data-[state=open]:bg-accent data-[state=open]:text-muted-foreground absolute end-4 top-4 rounded-xs opacity-70 transition-opacity hover:opacity-100 disabled:pointer-events-none [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-icon-md"
          >
            <X />
            <span className="sr-only">Close</span>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPortal>
  );
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  );
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean;
}) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn("flex flex-row justify-end gap-2", className)}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close asChild>
          <Button variant="outline">Close</Button>
        </DialogPrimitive.Close>
      )}
    </div>
  );
}

function DialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("font-display text-base leading-none font-medium", className)}
      {...props}
    />
  );
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn("text-muted-foreground text-sm", className)}
      {...props}
    />
  );
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPlainContent,
  DialogPlainOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
};
