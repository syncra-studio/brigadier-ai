import type { FC, ReactNode } from "react";

import { ChevronRight } from "@/components/icons";

/** A row's words and toggle, as the Workers strip draws its own: grey, its chevron after. */
export const StatusSlotToggle: FC<{ open: boolean; onToggle: () => void; icon: ReactNode; children: ReactNode }> = ({ open, onToggle, icon, children }) => (
  <button
    type="button"
    aria-expanded={open}
    onClick={onToggle}
    data-state={open ? "open" : "closed"}
    className="group/toggle hover:text-foreground text-foreground/60 rounded-control flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-start text-sm"
  >
    <span className="flex size-4 shrink-0 items-center justify-center">{icon}</span>
    <span className="flex min-w-0 items-center">
      <span className="min-w-0 truncate tabular-nums">{children}</span>
      <span aria-hidden className="size-icon-button-sm flex shrink-0 items-center justify-center">
        <ChevronRight className="text-row-chevron size-chevron group-hover/toggle:text-foreground group-focus-visible/toggle:text-foreground transition-[rotate,color] duration-300 ease-standard group-data-[state=open]/toggle:rotate-90 motion-reduce:transition-none" />
      </span>
    </span>
  </button>
);
