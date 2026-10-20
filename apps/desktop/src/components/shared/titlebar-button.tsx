import type { ReactNode } from "react";

import { TooltipProvider } from "@/components/overlays/tooltip";
import { cn } from "@/lib/utils";

/** How long the pointer rests on a titlebar button before its tip shows; the next tip in the
 * same group then shows at once. */
const TIP_DELAY_MS = 250;

/** The look of a titlebar icon button: TooltipIconButton's `toolbar` look, and the sidebar toggle. */
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
