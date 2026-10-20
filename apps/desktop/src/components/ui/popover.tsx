import type * as React from "react";
import { Popover as PopoverPrimitive } from "radix-ui";

import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";

function Popover(props: React.ComponentProps<typeof PopoverPrimitive.Root>) {
  return <PopoverPrimitive.Root data-slot="popover" {...props} />;
}

function PopoverTrigger(props: React.ComponentProps<typeof PopoverPrimitive.Trigger>) {
  return <PopoverPrimitive.Trigger data-slot="popover-trigger" {...props} />;
}

function PopoverAnchor(props: React.ComponentProps<typeof PopoverPrimitive.Anchor>) {
  return <PopoverPrimitive.Anchor data-slot="popover-anchor" {...props} />;
}

function PopoverContent({
  className,
  // One spacing step from the trigger.
  sideOffset = tokenPx("--spacing"),
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Content>) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content
        data-slot="popover-content"
        sideOffset={sideOffset}
        className={cn(
          "data-[side=bottom]:slide-in-from-top-2 data-[side=top]:slide-in-from-bottom-2 data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 bg-popover text-popover-foreground data-[state=closed]:animate-out data-[state=open]:animate-in ring-foreground/10 rounded-menu shadow-menu z-50 origin-(--radix-popover-content-transform-origin) p-1 ring-1 duration-100 outline-hidden",
          className,
        )}
        {...props}
      />
    </PopoverPrimitive.Portal>
  );
}

export { Popover, PopoverAnchor, PopoverContent, PopoverTrigger };
