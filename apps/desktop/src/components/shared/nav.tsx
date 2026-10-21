import { useId, type ReactNode } from "react";

import { ChevronDown } from "@/components/icons";
import { cn } from "@/lib/utils";

/*
 * The parts the sidebar panel is built from, whatever it shows (chats and projects, or the
 * Settings navigation): its header, sections with a title that fold their list away, and lists
 * of rows (NavRow, in controls).
 */

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
 * A section of the panel, titled unless it is set apart by its gap alone. With `onOpenChange` its title folds the list away (and opens
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
  title?: string | undefined;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  actions?: ReactNode;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={title ? id : undefined} className="group/section flex flex-col">
      {title && (
        <div className="flex min-h-6 items-center gap-1 ps-2 pe-1.5 pb-1">
          {onOpenChange ? (
            <button
              id={id}
              type="button"
              aria-expanded={open}
              onClick={() => onOpenChange(!open)}
              className="text-foreground/50 flex min-w-0 items-center gap-1 rounded-xs text-sm font-medium opacity-75"
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
      )}
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
