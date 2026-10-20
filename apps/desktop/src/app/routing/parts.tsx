import type { ReactNode } from "react";

import { ChevronRight, ChevronUp, MinusCircle, Upgrade } from "@/components/icons";
import { Badge as ChipBadge } from "@/components/controls/badge";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Routing is built from, laid out as the user's design has it and drawn with
 * every Settings page's parts: a section (a heading over a card), a card of rows split by
 * hairlines, a row's words, selects and choices, badges, the page's buttons and notes.
 */

/** A divider line drawn inside the top or bottom edge, as the design's strokes are: it adds no height. */
export const HAIRLINE_TOP = "before:bg-divider relative before:absolute before:inset-x-0 before:top-0 before:h-px";
export const HAIRLINE_BOTTOM = "after:bg-divider relative after:absolute after:inset-x-0 after:bottom-0 after:h-px";

/** A row that opens in place: right when closed, up when open. */
export function OpenChevron({ open }: { open: boolean }) {
  const Icon = open ? ChevronUp : ChevronRight;
  return <Icon aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />;
}

/** "Never" in a rule: a circle struck through. */
export function BanIcon({ className }: { className?: string }) {
  return <MinusCircle aria-hidden className={cn("text-destructive size-icon-sm shrink-0 rotate-45", className)} />;
}

/** "Prefer" in a rule: an arrow up, in blue. */
export function PreferIcon({ className }: { className?: string }) {
  return <Upgrade aria-hidden className={cn("text-link size-icon-sm shrink-0", className)} />;
}

/** A small rounded mark: green (Picked now) or a plain one (New). */
export function Badge({ tone, children }: { tone: "success" | "link"; children: ReactNode }) {
  return (
    <ChipBadge variant={tone === "success" ? "success" : "secondary"} className="shrink-0">
      {children}
    </ChipBadge>

  );
}

/** A note inset in a card: what a change would do, or (warning) why it would do nothing. */
export function Note({ tone, icon, children }: { tone: "info" | "warning"; icon: ReactNode; children: ReactNode }) {
  return (
    <div
      role={tone === "warning" ? "alert" : "status"}
      className={cn(
        "rounded-control flex gap-2 px-3 py-2.5 [&>svg]:mt-0.5 [&>svg]:size-icon-sm [&>svg]:shrink-0",
        tone === "info" ? "bg-foreground/5 text-foreground/65" : "bg-warning/10 text-warning",
      )}
    >
      {icon}
      <p className="min-w-0 flex-1 text-xs">{children}</p>
    </div>
  );
}

/** A numbered step's number, in a ring. */
export function StepNumber({ children }: { children: ReactNode }) {
  return (
    <span className="border-divider text-muted-foreground text-2xs flex size-5.5 shrink-0 items-center justify-center rounded-full border font-mono">
      {children}
    </span>
  );
}

