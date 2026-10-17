import type * as React from "react";
import { Tooltip as TooltipPrimitive } from "radix-ui";

import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";

function TooltipProvider({
  delayDuration = 0,
  // A tooltip closes once the pointer leaves its control. A hoverable one stays open while the
  // pointer is on the way to it, and blocks the others meanwhile: a control next to it showed
  // the old tooltip instead of its own. Nor does the pointer land on one (pointer-events-none
  // below): a tooltip sliding in over its own control would close it.
  disableHoverableContent = true,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      disableHoverableContent={disableHoverableContent}
      {...props}
    />
  );
}

function Tooltip({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Root>) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />;
}

function TooltipTrigger({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />;
}

function TooltipContent({
  className,
  // One spacing step, so the gap to the trigger follows density.
  sideOffset = tokenPx("--spacing"),
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        sideOffset={sideOffset}
        className={cn(
          "pointer-events-none fade-in-0 zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 animate-in bg-popover text-popover-foreground border-border border shadow-tooltip data-[state=closed]:animate-out rounded-tooltip tracking-tooltip z-50 inline-flex w-fit max-w-xs origin-(--radix-tooltip-content-transform-origin) items-center gap-1.5 px-3 py-1.25 text-center text-sm has-data-[slot=kbd]:pe-1.5 **:data-[slot=kbd]:rounded-full",
          className,
        )}
        {...props}
      >
        {children}
      </TooltipPrimitive.Content>
    </TooltipPrimitive.Portal>
  );
}

export { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider };
