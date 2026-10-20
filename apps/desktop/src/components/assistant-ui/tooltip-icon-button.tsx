import { type ComponentPropsWithRef, forwardRef } from "react";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Slot } from "@/components/ui/slot";
import { cn } from "@/lib/utils";

export type TooltipIconButtonProps = ComponentPropsWithRef<typeof Button> & {
  tooltip: string;
  /** The button's keyboard shortcut, as a pill after the tip (e.g. "Dictate ⌃⇧D"). */
  shortcut?: string | undefined;
  side?: "top" | "bottom" | "left" | "right";
};

export const TooltipIconButton = forwardRef<
  HTMLButtonElement,
  TooltipIconButtonProps
>(
  (
    { children, tooltip, shortcut, side = "bottom", size = "icon-sm", className, ...rest },
    ref,
  ) => {
    return (
      <TooltipProvider delayDuration={0}>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size={size}
              {...rest}
              className={cn(
                "aui-button-icon active:scale-90",
                // Quiet controls: a light wash while hovered or open.
                (rest.variant ?? "ghost") === "ghost" &&
                  "hover:bg-foreground/8 data-[state=open]:bg-foreground/8",
                className,
              )}
              ref={ref}
            >
              <Slot.Slottable>{children}</Slot.Slottable>
              <span className="aui-sr-only sr-only">{tooltip}</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent side={side}>
            {tooltip}
            {shortcut && <Kbd>{shortcut}</Kbd>}
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
    );
  },
);

TooltipIconButton.displayName = "TooltipIconButton";
