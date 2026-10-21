import type { ReactNode } from "react";

/** A labelled field: its label (and a note on the right), then the control. */
export function Field({ label, aside, htmlFor, children }: { label: string; aside?: string; htmlFor?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-4">
        <label htmlFor={htmlFor} className="text-foreground text-label leading-4 font-medium">
          {label}
        </label>
        {aside && <span className="text-muted-foreground text-xs leading-3.75">{aside}</span>}
      </div>
      {children}
    </div>
  );
}
