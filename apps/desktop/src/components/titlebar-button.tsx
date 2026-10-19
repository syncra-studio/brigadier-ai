import { type ComponentPropsWithRef, forwardRef, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** How long the pointer rests on a titlebar button before its tip shows; the next tip in the
 * same group then shows at once. */
const TIP_DELAY_MS = 250;

/** The look of a titlebar icon button, for buttons that bring their own behaviour. */
export const TITLEBAR_BUTTON = cn(
  "rounded-toolbar-button text-toolbar-foreground hover:bg-toolbar-hover hover:text-foreground focus-visible:text-foreground data-[state=open]:text-foreground data-[state=open]:bg-toolbar-hover active:bg-toolbar-active [&_svg]:size-icon-md transition-colors",
  "aria-pressed:bg-toolbar-pressed aria-pressed:text-foreground aria-pressed:hover:bg-toolbar-pressed-hover",
);

/**
 * The titlebar's icon buttons share one tip timing: wrap a group of them in this.
 */
export function TitlebarTips({ children }: { children: ReactNode }) {
  return <TooltipProvider delayDuration={TIP_DELAY_MS}>{children}</TooltipProvider>;
}

export type TitlebarButtonProps = ComponentPropsWithRef<typeof Button> & {
  tooltip: string;
  /** The button's keyboard shortcut, shown after the tip. */
  shortcut?: string | undefined;
};

/**
 * An icon button in the titlebar: a dim glyph that washes on hover and while held, and
 * shows as switched on (full glyph, a faint fill) while `aria-pressed`.
 */
export const TitlebarButton = forwardRef<HTMLButtonElement, TitlebarButtonProps>(
  ({ tooltip, shortcut, className, children, ...rest }, ref) => (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          ref={ref}
          variant="ghost"
          size="icon-md"
          aria-label={tooltip}
          {...rest}
          className={cn(TITLEBAR_BUTTON, className)}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom">
        {tooltip}
        {shortcut && <Kbd>{shortcut}</Kbd>}
      </TooltipContent>
    </Tooltip>
  ),
);

TitlebarButton.displayName = "TitlebarButton";
