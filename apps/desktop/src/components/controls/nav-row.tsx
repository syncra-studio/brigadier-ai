import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";

const navRow =
  "h-nav-row rounded-nav text-foreground/85 hover:bg-foreground/8 aria-[current=page]:bg-foreground/8 data-[active=true]:bg-foreground/8 relative flex w-full min-w-0 items-center gap-2 px-2 text-start text-sm [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-icon-md";

/** A navigation row: icon, label, then anything at its end. Hovered or selected, it is filled. */
export function NavRow({ className, ...props }: ComponentProps<"button">) {
  return <button type="button" className={cn(navRow, className)} {...props} />;
}
