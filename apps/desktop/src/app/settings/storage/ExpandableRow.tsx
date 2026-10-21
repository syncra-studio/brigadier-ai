import type { ReactNode } from "react";

import { StorageSize } from "@/app/settings/storage/StorageSize";
import type { Part } from "@/app/settings/storage/model";
import { ChevronDown, ChevronRight } from "@/components/icons";
import { UsageBar } from "@/components/shared";
import { cn } from "@/lib/utils";

/** Each part's share of everything Brigadier uses, for a project's 200-wide bar. */
export function shares(parts: readonly Part[], total: number) {
  return parts
    .filter((part) => part.bytes > 0)
    .map((part) => ({ key: part.key, value: total > 0 ? (part.bytes / total) * 100 : 0, className: part.color }));
}

/** All of `parts` together. */
export const sumBytes = (parts: readonly Part[]) => parts.reduce((total, part) => total + part.bytes, 0);

/** A row that opens: icon, name (and line), share bar, size, chevron; open, its detail. */
export function ExpandableRow({
  name,
  compact = false,
  icon,
  title,
  line,
  parts,
  total,
  open,
  onToggle,
  children,
}: {
  name: string;
  compact?: boolean;
  icon: ReactNode;
  title: ReactNode;
  line?: string | undefined;
  parts: Part[];
  total: number;
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <div data-project={name} className={cn("flex flex-col", compact && "border-divider border-b")}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className={cn("flex w-full items-center gap-4 text-left", compact ? "h-12" : "py-3.5")}
      >
        {icon}
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          {title}
          {line && <span className="text-muted-foreground truncate text-xs">{line}</span>}
        </span>
        <UsageBar parts={shares(parts, total)} size="md" className="w-50" />
        <StorageSize bytes={sumBytes(parts)} />
        {open ? (
          <ChevronDown aria-hidden className="text-muted-foreground size-3.75 shrink-0" />
        ) : (
          <ChevronRight aria-hidden className="text-muted-foreground size-3.75 shrink-0" />
        )}
      </button>
      {open && <div className="flex flex-col pb-3.5 ps-8">{children}</div>}
    </div>
  );
}
