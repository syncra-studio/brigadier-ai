import { Check, ChevronRight, Minus } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import type { Part } from "@/app/settings/storage/model";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Storage is built from, drawn to the user's own design: a section (its
 * title, a line under it and a note on the right, over its card), a card, pill buttons, a blue
 * link, a round badge, a tick box, the usage bar and a project's share bar. Its colours and
 * type are the redesigned Settings pages' (`settings-*`) and Storage's own (`storage-*`).
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
    <section data-setting={setting ?? title} aria-label={title} className="flex flex-col gap-3.5">
      <div className="flex items-end gap-6">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h2 className="text-settings-text text-settings-heading font-medium">{title}</h2>
          {description && <p className="text-settings-text-secondary text-settings-small">{description}</p>}
        </div>
        {aside && <div className="text-settings-text-tertiary shrink-0 text-settings-small">{aside}</div>}
      </div>
      {children}
    </section>
  );
}

/** A card: rows 20 in from its edges (`padded` pads the whole card, the usage card). */
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
        "bg-settings-surface border-settings-border rounded-storage-card flex flex-col border",
        padded ? "gap-4.5 p-5" : "px-5",
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
      <span className="text-settings-text text-settings-body font-medium">{title}</span>
      {line && <span className="text-settings-text-secondary text-settings-small">{line}</span>}
    </div>
  );
}

/** A grey pill button: Refresh, Review, Manage, Reveal in Finder, Free up. */
export function Pill({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "bg-settings-raised border-settings-ring text-settings-text hover:bg-settings-chosen rounded-capsule flex h-7.5 shrink-0 items-center gap-1.5 border px-3.5 text-settings-small font-medium transition-colors disabled:opacity-50 [&_svg]:size-3.25",
        className,
      )}
    />
  );
}

/** The light pill that confirms: Free up 39.7 MB. */
export function PrimaryPill({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "bg-settings-text text-settings-on-light rounded-capsule flex h-7.5 shrink-0 items-center px-3.5 text-settings-small font-medium transition-opacity hover:opacity-90 disabled:opacity-50",
        className,
      )}
    />
  );
}

/** The quiet pill beside it: Cancel. */
export function QuietPill({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "text-settings-text-secondary hover:text-settings-text rounded-capsule flex h-7.5 shrink-0 items-center px-3 text-settings-small font-medium transition-colors disabled:opacity-50",
        className,
      )}
    />
  );
}

/** The red pill: Uninstall…. */
export function DangerPill({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "bg-storage-danger border-storage-danger-edge text-storage-danger-text rounded-capsule flex h-7.5 shrink-0 items-center border px-3.5 text-settings-small font-medium transition-opacity hover:opacity-90",
        className,
      )}
    />
  );
}

/** A blue link with its chevron: Show all, Show 9 more, What was removed. */
export function LinkButton({
  icon: Icon = ChevronRight,
  className,
  children,
  ...props
}: { icon?: typeof ChevronRight } & ComponentProps<"button">) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "text-settings-link flex shrink-0 items-center gap-1 text-settings-small font-medium hover:underline",
        className,
      )}
    >
      {children}
      <Icon aria-hidden className="size-3.25" />
    </button>
  );
}

/** The round badge at a summary's start. */
export function RoundBadge({ tone = "plain", children }: { tone?: "plain" | "done"; children: ReactNode }) {
  return (
    <span
      aria-hidden
      className={cn(
        "flex size-9 shrink-0 items-center justify-center rounded-full [&_svg]:size-4",
        tone === "done" ? "bg-settings-success-soft text-settings-success" : "bg-settings-raised text-settings-text",
      )}
    >
      {children}
    </span>
  );
}

/** A tick box: ticked, partly ticked (a group), or not. */
export function TickBox({
  state,
  label,
  onToggle,
}: {
  state: "all" | "some" | "none";
  label: string;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={state === "all" ? true : state === "some" ? "mixed" : false}
      aria-label={label}
      onClick={onToggle}
      className={cn(
        "rounded-storage-box flex size-4 shrink-0 items-center justify-center",
        state === "none" ? "shadow-storage-box" : "bg-settings-text text-settings-on-light",
      )}
    >
      {state === "all" && <Check aria-hidden className="size-3" />}
      {state === "some" && <Minus aria-hidden className="size-3" />}
    </button>
  );
}

/** A part's coloured dot, as the legend and an opened project show it. */
export function Dot({ color }: { color: string }) {
  return <span aria-hidden className={cn("size-1.75 shrink-0 rounded-full", color)} />;
}

/** The usage bar: each part's share of the total, 2 apart; a part too small to see still shows. */
export function UsageBar({ parts }: { parts: readonly Part[] }) {
  const shown = parts.filter((part) => part.bytes > 0);
  return (
    <div aria-hidden className="bg-settings-track flex h-2.5 w-full gap-0.5 overflow-hidden rounded-full">
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
    <div aria-hidden className="bg-settings-track flex h-1.5 w-50 shrink-0 overflow-hidden rounded-full">
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
