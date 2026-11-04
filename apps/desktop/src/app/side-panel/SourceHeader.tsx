import type { ReactNode } from "react";

import { Branch, Check, ChevronDown, ChevronRight, DotsHorizontal, Reload } from "@/components/icons";
import { TooltipIconButton } from "@/components/controls";
import { DropdownMenu, DropdownMenuTrigger } from "@/components/overlays";
import { cn } from "@/lib/utils";

/**
 * The Source panel's title row: SOURCE and, with one checkout, its actions, always in view. With
 * stacked rows it has none: each row has its own.
 */
export function SourceHeader({ children }: { children?: ReactNode }) {
  return (
    <header data-slot="source-title" className="text-muted-foreground flex h-control-md shrink-0 items-center gap-0.5 ps-3 pe-2">
      <span className="min-w-0 flex-1 text-xs font-semibold tracking-wide">SOURCE</span>
      {children}
    </header>
  );
}

/** A checkout's actions: Commit (off when it can't), Refresh, then More Actions (`moreMenu`), where the checkout can change. */
export function SourceActions({ canCommit, locked, onCommit, onRefresh, moreMenu }: {
  canCommit: boolean;
  locked: boolean;
  onCommit: () => void;
  onRefresh: () => void;
  moreMenu: ReactNode;
}) {
  return (
    <>
      <TooltipIconButton tooltip="Commit" size="icon-xs" disabled={locked || !canCommit} onClick={onCommit}>
        <Check />
      </TooltipIconButton>
      <SourceRefresh onRefresh={onRefresh} />
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <TooltipIconButton tooltip="More Actions" size="icon-xs" disabled={locked}>
            <DotsHorizontal />
          </TooltipIconButton>
        </DropdownMenuTrigger>
        {moreMenu}
      </DropdownMenu>
    </>
  );
}

export function SourceRefresh({ onRefresh }: { onRefresh: () => void }) {
  return (
    <TooltipIconButton tooltip="Refresh" size="icon-xs" onClick={onRefresh}>
      <Reload />
    </TooltipIconButton>
  );
}

/**
 * One of a Source's stacked rows: the fold arrow, the checkout's name, its branch, its actions.
 * The name gets room before the branch does; both truncate, with their full text on hover.
 */
export function SourceRow({ open, onToggle, icon, name, title, branch, branchTitle, onBranch, branchDisabled, children }: {
  open: boolean;
  onToggle: () => void;
  icon?: ReactNode;
  name: ReactNode;
  title: string;
  branch: string | null;
  branchTitle?: string;
  /** Opens Checkout to…; null where the branch can't change. */
  onBranch?: (() => void) | null;
  branchDisabled?: boolean;
  children: ReactNode;
}) {
  const look = "text-muted-foreground flex min-w-16 shrink-[100] items-center gap-1 text-sm";
  const chip = (
    <>
      <Branch className="size-icon-sm shrink-0" />
      <span className="min-w-0 truncate">{branch ?? "…"}</span>
    </>
  );
  return (
    <header data-slot="source-row" className="flex h-control-md shrink-0 items-center gap-1 ps-1 pe-1">
      <button type="button" aria-expanded={open} aria-label={`Fold ${title}`} onClick={onToggle} className="text-muted-foreground shrink-0">
        {open ? <ChevronDown className="size-icon-sm" /> : <ChevronRight className="size-icon-sm" />}
      </button>
      <button type="button" title={title} onClick={onToggle} className="flex min-w-0 shrink items-center gap-1.5 text-start text-sm font-semibold">
        {icon}
        <span data-slot="source-row-name" className="min-w-0 truncate">{name}</span>
      </button>
      {onBranch ? (
        <button type="button" data-slot="source-row-branch" title={branchTitle ?? branch ?? undefined} disabled={branchDisabled} onClick={onBranch}
          className={cn(look, "hover:text-foreground disabled:opacity-50")}>
          {chip}
        </button>
      ) : (
        <span data-slot="source-row-branch" title={branchTitle ?? branch ?? undefined} className={look}>
          {chip}
        </span>
      )}
      <span className="ms-auto flex shrink-0 items-center">{children}</span>
    </header>
  );
}
