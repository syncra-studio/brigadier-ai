import type { FC, ReactNode } from "react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";

/** One of the capsule's pills: its words, what shows on hover, and a click when it has a `label`. */
export const CapsulePill: FC<{ tip: ReactNode; label?: string; onClick?: () => void; children: ReactNode }> = ({ tip, label, onClick, children }) => (
  <Tooltip delayDuration={0}>
    <TooltipTrigger asChild>
      {label ? (
        // A plan's checklist, or the changes: reachable from the keyboard too.
        <button
          type="button"
          aria-label={label}
          onClick={onClick}
          className="hover:text-foreground rounded-control flex min-w-0 items-center gap-1.5 text-start transition-colors"
        >
          {children}
        </button>
      ) : (
        <span className="flex items-center gap-1.5">{children}</span>
      )}
    </TooltipTrigger>
    <TooltipContent side="top" className="flex-col items-stretch gap-1 py-1.5">
      {tip}
    </TooltipContent>
  </Tooltip>
);
