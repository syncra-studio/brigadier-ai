import type { ReactNode } from "react";

/** What an overnight run came to, under its folded work once the run is over. */
export function PhaseOutcome({ children }: { children: ReactNode }) {
  return (
    <p data-slot="phase-outcome" className="text-foreground pt-activity text-sm leading-relaxed wrap-break-word">
      {children}
    </p>
  );
}
