import { ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { useId, type ComponentProps, type ReactNode } from "react";

import { Kbd } from "@/components/ui/kbd";
import { useSidebar } from "@/components/ui/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";

/*
 * The parts the sidebar panel is built from, whatever it shows (chats and projects, or the
 * Settings navigation): its header, sections with a title that fold their list away, and rows.
 */

/** A navigation row: icon, label, then anything at its end. Hovered or selected, it is filled. */
export const navRow =
  "h-nav-row rounded-nav text-foreground/85 hover:bg-foreground/8 focus-visible:ring-ring/50 aria-[current=page]:bg-foreground/8 data-[active=true]:bg-foreground/8 relative flex w-full min-w-0 items-center gap-2 px-2 text-start text-sm outline-none focus-visible:ring-2 [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-icon-md";

/** An icon button on a row's end, shown while the row is hovered or focused. */
export const rowAction =
  "text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 flex size-icon-button-xs shrink-0 items-center justify-center rounded-xs outline-none focus-visible:ring-2 [&_svg]:size-icon-md";

/** The panel's header: its title, and icon buttons at the end. */
export function NavHeader({ title, actions }: { title: ReactNode; actions?: ReactNode }) {
  return (
    <div className="h-nav-header flex shrink-0 items-center gap-1 px-2">
      <h1 className="text-nav-title min-w-0 flex-1 truncate ps-2 font-semibold">{title}</h1>
      {actions && <div className="flex shrink-0 items-center gap-1">{actions}</div>}
    </div>
  );
}

/**
 * A titled section of the panel. With `onOpenChange` its title folds the list away (and opens
 * it again): the list's height and opacity ease over 300ms, the chevron turns in 150ms.
 * `actions` show at the header's end while the section is hovered.
 */
export function NavSection({
  title,
  open = true,
  onOpenChange,
  actions,
  children,
}: {
  title: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  actions?: ReactNode;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="group/section flex flex-col">
      <div className="flex min-h-6 items-center gap-1 ps-2 pe-1.5 pb-1">
        {onOpenChange ? (
          <button
            id={id}
            type="button"
            aria-expanded={open}
            onClick={() => onOpenChange(!open)}
            className="text-foreground/50 focus-visible:ring-ring/50 flex min-w-0 items-center gap-1 rounded-xs text-sm font-medium opacity-75 outline-none focus-visible:ring-2"
          >
            <span className="truncate">{title}</span>
            <ChevronDown
              aria-hidden
              className={cn(
                "ease-standard size-icon-sm shrink-0 transition-[rotate,opacity] duration-150 motion-reduce:transition-none",
                open
                  ? "opacity-0 group-focus-within/section:opacity-100 group-hover/section:opacity-100"
                  : "-rotate-90",
              )}
            />
          </button>
        ) : (
          <h2 id={id} className="text-foreground/50 truncate text-sm font-medium opacity-75">
            {title}
          </h2>
        )}
        {actions && (
          <div className="ms-auto flex shrink-0 items-center gap-0.5 opacity-0 group-focus-within/section:opacity-100 group-hover/section:opacity-100">
            {actions}
          </div>
        )}
      </div>
      <NavFold open={open}>{children}</NavFold>
    </section>
  );
}

/** Content that folds away to nothing and opens again: height and opacity ease over 300ms. */
export function NavFold({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div
      inert={!open}
      className={cn(
        "ease-enter grid transition-[grid-template-rows,opacity] duration-300 motion-reduce:transition-none",
        open ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
      )}
    >
      <div className="min-h-0 overflow-hidden">{children}</div>
    </div>
  );
}

/** A list of rows, a hair apart. */
export function NavList({ children, className }: { children: ReactNode; className?: string }) {
  return <ul className={cn("flex flex-col gap-px", className)}>{children}</ul>;
}

/** A muted line in a list with nothing to show ("No sessions yet"). */
export function NavEmpty({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <li className={cn("text-foreground/35 h-nav-row flex items-center px-2 text-sm", className)}>
      {children}
    </li>
  );
}

/**
 * How far a menu opened beside the collapsed strip sits from its icon: past the strip's edge
 * (the strip's inset), then the usual gap.
 */
export function besideStrip(): number {
  return tokenPx("--spacing") * 3;
}

/** Where a foot icon's menu opens: above it, or beside the strip while collapsed. */
export function footMenuPlacement(expanded: boolean) {
  return expanded
    ? ({ side: "top", align: "start" } as const)
    : ({ side: "right", align: "end", sideOffset: besideStrip() } as const);
}

/**
 * An icon button on the strip or at the sidebar's foot: a muted glyph that brightens on hover,
 * filled while hovered, selected or while its menu is open. Its corners are only slightly
 * rounded, so the small square never reads as a pill or a circle.
 */
const stripItem = (selected: boolean) =>
  cn(
    "h-nav-row rounded-sm focus-visible:ring-ring/50 relative flex shrink-0 items-center justify-center outline-none transition-colors duration-150 focus-visible:ring-2 [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-icon-md",
    "hover:bg-foreground/8 data-[state=open]:bg-foreground/8",
    selected
      ? "bg-foreground/8 text-foreground"
      : "text-foreground/85 hover:text-foreground data-[state=open]:text-foreground",
  );

/**
 * An icon at the sidebar's foot (Keep awake, Usage, Updates, Settings): in a row along its
 * bottom while the sidebar is expanded, stacked where the strip's icons sit while it is
 * collapsed, the same size either way. Its name and how it stands (`tip`, and shortcut) show in
 * a tooltip above it, or to its right on the strip. `dot` marks the icon (a background colour
 * class); `end` follows the icon (the Updates pill's marks).
 */
export function FootButton({
  tip,
  shortcut,
  icon,
  dot,
  end,
  selected = false,
  className,
  ...props
}: ComponentProps<"button"> & {
  tip: string;
  shortcut?: string | undefined;
  icon: ReactNode;
  dot?: string | null | undefined;
  end?: ReactNode;
  selected?: boolean;
}) {
  const { open } = useSidebar();
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={tip}
          aria-pressed={selected || undefined}
          className={cn(stripItem(selected), "min-w-strip-button gap-1.5", className)}
          {...props}
        >
          <span className="relative flex shrink-0">
            {icon}
            {dot && (
              <span
                aria-hidden
                className={cn("absolute -end-0.5 -top-0.5 size-1.5 rounded-full", dot)}
              />
            )}
          </span>
          {end}
        </button>
      </TooltipTrigger>
      <TooltipContent side={open ? "top" : "right"}>
        {tip}
        {shortcut && <Kbd>{shortcut}</Kbd>}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * An icon on the collapsed sidebar's strip, where its expanded row's icon sits, its name (and
 * shortcut) in a tooltip to its right.
 */
export function StripButton({
  label,
  shortcut,
  selected = false,
  children,
  className,
  ...props
}: ComponentProps<"button"> & { label: string; shortcut?: string | undefined; selected?: boolean }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={label}
          aria-pressed={selected || undefined}
          className={cn(stripItem(selected), "w-full", className)}
          {...props}
        >
          {children}
        </button>
      </TooltipTrigger>
      <TooltipContent side="right">
        {label}
        {shortcut && <Kbd>{shortcut}</Kbd>}
      </TooltipContent>
    </Tooltip>
  );
}
