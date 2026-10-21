import type { ReactNode } from "react";

import { CHEVRON, ROW, ROW_TOGGLE } from "@/components/conversation";
import { ChevronRightAlt } from "@/components/icons";
import { cn } from "@/lib/utils";

/** A finished answer's one fold, "Details", closed at first: what the answer keeps under it. */
export function DetailsFold({ children }: { children: ReactNode }) {
  return (
    <details data-slot="report-details" className="group/details">
      {/* Folds like a work row: its words, then the same quiet chevron, never the browser's marker. */}
      <summary className={cn(ROW, ROW_TOGGLE, "list-none [&::-webkit-details-marker]:hidden")}>
        Details
        <ChevronRightAlt aria-hidden className={cn(CHEVRON, "group-open/details:rotate-90")} />
      </summary>
      <div className="pt-2">
        {children}
      </div>
    </details>
  );
}
