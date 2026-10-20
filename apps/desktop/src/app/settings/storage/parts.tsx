import type { ComponentProps, ReactNode } from "react";

import { settingsCard, SettingsSection } from "@/app/settings/parts";
import type { Part } from "@/app/settings/storage/model";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Storage is built from, laid out as the user's design has it and drawn with
 * every Settings page's parts: a section (its title, a line under it and a note on the right,
 * over its card), a card, buttons, a blue link, a round badge, a tick box, the usage bar and a
 * project's share bar.
 */

/** A section: its title (and a line under it), an optional note on the right, its card. */
export function StorageSection({
  title,
  description,
  aside,
  setting,
  children,
}: {
  title: string;
  description?: string;
  aside?: ReactNode;
  /** The name Settings search finds the section by, when not its title. */
  setting?: string;
  children: ReactNode;
}) {
  return (
    <div data-setting={setting ?? title}>
      <SettingsSection
        title={title}
        description={description}
        actions={aside && <div className="text-foreground/65 text-label">{aside}</div>}
      >
        {children}
      </SettingsSection>
    </div>
  );
}

/** A card: rows 16 in from its edges (`padded` pads the whole card, the usage card). */
export function StorageCard({
  padded = false,
  className,
  children,
  ...props
}: { padded?: boolean } & ComponentProps<"div">) {
  return (
    <div
      data-slot="storage-card"
      {...props}
      className={cn(
        settingsCard,
        padded ? "gap-4.5 p-4" : "px-4",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** A row's title and the line under it. */
export function RowText({ title, line, className }: { title: ReactNode; line?: ReactNode; className?: string }) {
  return (
    <div className={cn("flex min-w-0 flex-1 flex-col gap-1", className)}>
      <span className="text-label font-medium">{title}</span>
      {line && <span className="text-foreground/65 text-xs">{line}</span>}
    </div>

  );
}

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

/** A project's share of everything Brigadier uses: its parts on a 200-wide track. */
export function ShareBar({ parts, total }: { parts: readonly Part[]; total: number }) {
  return (
    <div aria-hidden className="bg-muted flex h-1.5 w-50 shrink-0 overflow-hidden rounded-full">
      {parts
        .filter((part) => part.bytes > 0)
        .map((part) => (
          <span
            key={part.key}
            data-part={part.key}
            className={cn("h-full min-w-0.5 shrink-0", part.color)}
            style={{ width: `${total > 0 ? (part.bytes / total) * 100 : 0}%` }}
          />
        ))}
    </div>
  );
}
