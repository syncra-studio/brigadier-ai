import { type ComponentPropsWithRef, forwardRef } from "react";

import { TITLEBAR_BUTTON } from "@/components/shared/titlebar-button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/overlays/tooltip";
import { Button } from "@/components/controls/button";
import { Kbd } from "@/components/controls/kbd";
import { Slot } from "@/components/controls/slot";
import { cn } from "@/lib/utils";

/**
 * A sidebar strip or foot icon: a row tall, a muted glyph that brightens on hover, filled while
 * hovered, pressed or while its menu is open.
 */
const STRIP_BUTTON = cn(
  "h-nav-row min-w-strip-button rounded-strip-button relative flex gap-1.5 transition-colors duration-150",
  "text-foreground/85 hover:text-foreground hover:bg-foreground/8 data-[state=open]:text-foreground data-[state=open]:bg-foreground/8",
  "aria-pressed:bg-foreground/8 aria-pressed:text-foreground",
);

export type TooltipIconButtonProps = ComponentPropsWithRef<typeof Button> & {
  tooltip: string;
  /** The button's keyboard shortcut, as a pill after the tip (e.g. "Dictate ⌃⇧D"). */
  shortcut?: string | undefined;
  side?: "top" | "bottom" | "left" | "right";
  /**
   * `ghost`: a quiet icon in a card, row or composer; its tip shows at once. `toolbar`: a
   * titlebar or pane-toolbar icon, the dim glyph that shows pressed with a faint fill; `strip`:
   * an icon on the collapsed sidebar's strip or at its foot. Those two take their tip timing
   * from the group around them (TitlebarTips, the sidebar) and are named by `tooltip`.
   */
  look?: "ghost" | "toolbar" | "strip";
  /** Switched on (aria-pressed). */
  pressed?: boolean | undefined;
  /** A small dot on the icon, in this background colour class ("bg-warning"). */
  dot?: string | null | undefined;
  /** `false` leaves the tip out (the strip's mark, whose hover opens the sidebar instead). */
  tip?: boolean;
};

export const TooltipIconButton = forwardRef<
  HTMLButtonElement,
  TooltipIconButtonProps
>(
  (
    {
      children,
      tooltip,
      shortcut,
      side = "bottom",
      look = "ghost",
      pressed,
      dot,
      tip = true,
      size,
      className,
      ...rest
    },
    ref,
  ) => {
    const ghost = look === "ghost";
    const button = (
      <Button
        variant={look === "strip" ? null : "ghost"}
        size={look === "strip" ? null : (size ?? (ghost ? "icon-sm" : "icon-md"))}
        aria-label={ghost ? undefined : tooltip}
        aria-pressed={pressed || undefined}
        {...rest}
        className={cn(
          ghost && "aui-button-icon active:scale-90",
          // Quiet controls: a light wash while hovered or open.
          ghost && (rest.variant ?? "ghost") === "ghost" &&
            "hover:bg-foreground/8 data-[state=open]:bg-foreground/8",
          look === "toolbar" && TITLEBAR_BUTTON,
          look === "strip" && STRIP_BUTTON,
          className,
        )}
        ref={ref}
      >
        <Slot.Slottable>
          {dot ? (
            <span className="relative flex shrink-0">
              {children}
              <span aria-hidden className={cn("absolute -end-0.5 -top-0.5 size-1.5 rounded-full", dot)} />
            </span>
          ) : (
            children
          )}
        </Slot.Slottable>
        {ghost && <span className="aui-sr-only sr-only">{tooltip}</span>}
      </Button>
    );
    if (!tip) return button;
    const tipped = (
      <Tooltip>
        <TooltipTrigger asChild>{button}</TooltipTrigger>
        <TooltipContent side={side}>
          {tooltip}
          {shortcut && <Kbd>{shortcut}</Kbd>}
        </TooltipContent>
      </Tooltip>
    );
    return ghost ? <TooltipProvider delayDuration={0}>{tipped}</TooltipProvider> : tipped;
  },
);

TooltipIconButton.displayName = "TooltipIconButton";
