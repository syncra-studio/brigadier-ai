import { type FC, type ReactNode, useState } from "react";

import { ChevronDown, ChevronRight } from "@/components/icons";

/** A group of files under its title and count, folding away; its actions show on hover. */
export const SourceGroup: FC<{ title: string; count: number; actions: ReactNode; children: ReactNode }> = ({
  title,
  count,
  actions,
  children,
}) => {
  const [open, setOpen] = useState(true);
  return (
    <section aria-label={title} className="mt-2">
      <header className="group/source-group flex h-control-sm items-center gap-1 pe-1 text-sm">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="flex min-w-0 flex-1 items-center gap-1 text-start"
        >
          {open ? (
            <ChevronDown className="text-muted-foreground size-icon-sm shrink-0" />
          ) : (
            <ChevronRight className="text-muted-foreground size-icon-sm shrink-0" />
          )}
          <span className="truncate">{title}</span>
        </button>
        <span className="hidden shrink-0 items-center group-focus-within/source-group:flex group-hover/source-group:flex">
          {actions}
        </span>
        <span className="bg-muted text-muted-foreground rounded-full px-1.5 text-xs tabular-nums">{count}</span>
      </header>
      {open && <ul>{children}</ul>}
    </section>
  );
};
