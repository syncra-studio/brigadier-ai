import type { ReactNode } from "react";

import type { Part } from "@/app/settings/storage/model";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Storage is built from, laid out as the user's design has it and drawn with
 * every Settings page's parts: a section (its title, a line under it and a note on the right,
 * over its card), a card, buttons, a blue link, a round badge, a tick box, the usage bar and a
 * project's share bar.
 */

/** The round badge at a summary's start. */
export function RoundBadge({ tone = "plain", children }: { tone?: "plain" | "done"; children: ReactNode }) {
  return (
    <span
      aria-hidden
      className={cn(
        "flex size-9 shrink-0 items-center justify-center rounded-full [&_svg]:size-icon-md",
        tone === "done" ? "bg-success/15 text-success" : "bg-foreground/8 text-foreground",
      )}
    >
      {children}
    </span>
  );
}

/** A part's coloured dot, as the legend and an opened project show it. */
export function Dot({ color }: { color: string }) {
  return <span aria-hidden className={cn("size-1.75 shrink-0 rounded-full", color)} />;
}

/** The usage bar: each part's share of the total, 2 apart; a part too small to see still shows. */
export function UsageBar({ parts, pending = false }: { parts: readonly Part[]; pending?: boolean }) {
  const shown = parts.filter((part) => part.bytes > 0);
  return (
    <div
      aria-hidden
      // Before the first scan the empty track holds the bar's room.
      data-slot={pending ? "storage-bar-pending" : "storage-bar"}
      className="bg-muted flex h-2.5 w-full gap-0.5 overflow-hidden rounded-full">
      {shown.map((part) => (
        <span
          key={part.key}
          data-part={part.key}
          className={cn("h-full min-w-0.75", part.color)}
          style={{ flexGrow: part.bytes, flexBasis: 0 }}
        />
      ))}
    </div>
  );
}
