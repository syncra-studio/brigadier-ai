import type { ReactNode } from "react";

import { Check, ChevronDown, ChevronRight, DotsHorizontal, Reload } from "@/components/icons";
import { TooltipIconButton } from "@/components/controls";
import { DropdownMenu, DropdownMenuTrigger } from "@/components/overlays";

/**
 * The Source panel's title row: SOURCE folds the panel; on hover, Commit and Refresh; then More
 * Actions (`moreMenu`), where the checkout can change.
 */
export function SourceHeader({ open, onToggle, readOnly, locked, onCommit, onRefresh, moreMenu }: {
  open: boolean;
  onToggle: () => void;
  readOnly: boolean;
  locked: boolean;
  onCommit: () => void;
  onRefresh: () => void;
  moreMenu: ReactNode;
}) {
  return (
    <header className="group/source-title text-muted-foreground flex h-control-md shrink-0 items-center gap-0.5 ps-1 pe-2">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="flex min-w-0 flex-1 items-center gap-1 text-start text-xs font-semibold tracking-wide"
      >
        {open ? <ChevronDown className="size-icon-sm shrink-0" /> : <ChevronRight className="size-icon-sm shrink-0" />}
        SOURCE
      </button>
      <span className="flex items-center opacity-0 transition-opacity group-hover/source-title:opacity-100 group-focus-within/source-title:opacity-100">
        {!readOnly && (
          <TooltipIconButton tooltip="Commit" size="icon-xs" disabled={locked} onClick={onCommit}>
            <Check />
          </TooltipIconButton>
        )}
        <TooltipIconButton tooltip="Refresh" size="icon-xs" onClick={onRefresh}>
          <Reload />
        </TooltipIconButton>
      </span>
      {!readOnly && (
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <TooltipIconButton tooltip="More Actions" size="icon-xs" disabled={locked}>
              <DotsHorizontal />
            </TooltipIconButton>
          </DropdownMenuTrigger>
          {moreMenu}
        </DropdownMenu>
      )}
    </header>
  );
}
