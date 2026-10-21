import { forwardRef, type ReactNode } from "react";

import { composerPill } from "@/components/conversation";
import { cn } from "@/lib/utils";

/** A rail trigger: the composer's pill, in the rail's bright text. */
export const RailPill = forwardRef<
  HTMLButtonElement,
  { icon: ReactNode; label: string; title?: string; children: ReactNode }
>(({ icon, label, title, children, ...props }, ref) => (
  <button
    ref={ref}
    type="button"
    aria-label={label}
    title={title}
    className={cn(composerPill, "text-foreground max-w-xs")}
    {...props}
  >
    {icon}
    <span className="truncate">{children}</span>
  </button>
));
RailPill.displayName = "RailPill";

