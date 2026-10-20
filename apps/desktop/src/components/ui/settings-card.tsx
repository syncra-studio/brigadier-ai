import type * as React from "react";

import { cn } from "@/lib/utils";

/** Rows split as a card's own are: a hairline under each but the last, inset 16 from the edges. */
const settingsRows =
  "*:not-last:relative *:not-last:after:pointer-events-none *:not-last:after:absolute *:not-last:after:inset-x-4 *:not-last:after:bottom-0 *:not-last:after:h-px *:not-last:after:bg-divider";

/**
 * A Settings card: every Settings page's fill, edge and corners, its rows split by hairlines.
 * Each row pads itself 16 in; `inset` pads the card instead, for content that isn't rows of its
 * own (a card's hairlines then still sit 16 in). `split={false}` leaves the hairlines out, for
 * content that draws its own. `padded` pads it 16 all round, its parts 18 apart (Storage's usage).
 */
function SettingsCard({
  inset = false,
  split = true,
  padded = false,
  className,
  ...props
}: React.ComponentProps<"div"> & { inset?: boolean; split?: boolean; padded?: boolean }) {
  return (
    <div
      data-slot="settings-card"
      {...props}
      className={cn(
        "bg-card border-divider rounded-settings flex flex-col overflow-hidden border",
        padded ? "gap-4.5 p-4" : inset && "px-4",
        split && !padded && settingsRows,
        split && inset && !padded && "*:not-last:after:inset-x-0",
        className,
      )}
    />
  );
}

/**
 * A row's words: its title over a grey line. `quiet` drops the title's weight (a sub-row's);
 * `truncate` keeps each to one line, cut with "…".
 */
function RowText({
  title,
  line,
  quiet = false,
  truncate = false,
  className,
}: {
  title: React.ReactNode;
  line?: React.ReactNode;
  quiet?: boolean;
  truncate?: boolean;
  className?: string;
}) {
  return (
    <span className={cn("flex min-w-0 flex-1 flex-col gap-1", className)}>
      <span className={cn("text-label", !quiet && "font-medium", truncate && "truncate")}>{title}</span>
      {line && <span className={cn("text-foreground/65 text-xs", truncate && "truncate")}>{line}</span>}
    </span>
  );
}

export { RowText, SettingsCard, settingsRows };