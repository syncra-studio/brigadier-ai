import { useState, type ReactNode } from "react";

import { besideStrip } from "@/app/shell/sidebar/nav";
import { TooltipIconButton } from "@/components/controls";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays";

/**
 * A strip icon that opens a list beside the strip, to pick from without expanding the sidebar.
 * It closes on a pick (`pick` runs the choice and closes it), on Esc and on a click outside.
 */
export function Flyout({
  label,
  icon,
  children,
}: {
  label: string;
  icon: ReactNode;
  children: (pick: (choose: () => void) => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const pick = (choose: () => void) => {
    setOpen(false);
    choose();
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <TooltipIconButton look="strip" side="right" tooltip={label} className="w-full">{icon}</TooltipIconButton>
      </PopoverTrigger>
      <PopoverContent
        side="right"
        align="start"
        sideOffset={besideStrip()}
        aria-label={label}
        // Focus the list itself, not its first row, so Tab goes on into the rows.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus();
        }}
        className="flex max-h-(--radix-popover-content-available-height) w-72 flex-col overflow-y-auto p-1.5"
      >
        {children(pick)}
      </PopoverContent>
    </Popover>
  );
}
