import type { ReactNode } from "react";

import { OpenChevron } from "@/app/settings/routing/parts";
import { RowText } from "@/components/shared";

/** One of More's rows: its icon, name and line, a summary, and what it opens to. */
export function MoreRowHead({
  row,
  icon,
  open,
  summary,
  onToggle,
  children,
}: {
  row: { label: string; description: string };
  icon: ReactNode;
  open: boolean;
  summary?: string | undefined;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <div data-setting={row.label} data-open={open || undefined} className="flex flex-col">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="flex w-full items-center gap-3.5 py-3.5 text-start [&>svg:first-child]:text-foreground/65 [&>svg:first-child]:size-4 [&>svg:first-child]:shrink-0"
      >
        {icon}
        <RowText truncate title={row.label} line={row.description} />
        {summary && !open && <span className="text-xs text-muted-foreground shrink-0">{summary}</span>}
        <OpenChevron open={open} />
      </button>
      {open && children}
    </div>
  );
}
