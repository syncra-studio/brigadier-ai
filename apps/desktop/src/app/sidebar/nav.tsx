import { ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { useId, type ComponentProps, type ReactNode } from "react";

import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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
 * A button on the bottom bar: a muted icon that brightens on hover, its name (and shortcut) in
 * a tooltip above it, a pill behind it while selected or while its menu is open.
 */
export function BarButton({
  label,
  shortcut,
  selected = false,
  children,
  className,
  ...props
}: ComponentProps<"button"> & { label: string; shortcut?: string; selected?: boolean }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={label}
          aria-pressed={selected || undefined}
          className={cn(
            "size-bar-button rounded-toolbar-button focus-visible:ring-ring/50 relative flex shrink-0 items-center justify-center outline-none transition-colors duration-150 focus-visible:ring-2 [&_svg]:relative [&_svg]:size-icon-md",
            "before:rounded-toolbar-button before:bg-foreground/8 before:absolute before:inset-0 before:opacity-0 before:transition-opacity before:duration-150 hover:before:opacity-100 data-[state=open]:before:opacity-100",
            selected ? "text-foreground before:opacity-100" : "text-muted-foreground hover:text-foreground data-[state=open]:text-foreground",
            className,
          )}
          {...props}
        >
          {children}
        </button>
      </TooltipTrigger>
      <TooltipContent side="top">
        {label}
        {shortcut && <Kbd>{shortcut}</Kbd>}
      </TooltipContent>
    </Tooltip>
  );
}
