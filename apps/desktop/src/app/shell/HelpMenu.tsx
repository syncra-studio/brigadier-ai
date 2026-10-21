import { useRef } from "react";

import { TooltipIconButton } from "@/components/controls";
import { footMenuPlacement } from "@/app/shell/sidebar/nav";
import { Comment, HelpIcon, Keyboard } from "@/components/icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { useSidebar } from "@/components/layout";
import { openSettings } from "@/state/actions";
import { openFeedback } from "@/state/feedback";

/** Help at the sidebar's foot: a life ring that opens Send feedback… and Keyboard shortcuts. */
export function HelpMenu() {
  const { open } = useSidebar();
  // Send feedback opens once the menu has closed, so its note keeps the keyboard rather than the
  // closing menu handing it back to Help.
  const toFeedback = useRef(false);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton look="strip" side={open ? "top" : "right"} tooltip="Help">
          <HelpIcon />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        {...(open ? ({ side: "top", align: "end" } as const) : footMenuPlacement(false))}
        // The design's width: sized to its two rows it read as an oval rather than a menu.
        className="w-60"
        onCloseAutoFocus={(event) => {
          if (!toFeedback.current) return;
          toFeedback.current = false;
          event.preventDefault();
          openFeedback();
        }}
      >
        <DropdownMenuItem
          onSelect={() => {
            toFeedback.current = true;
          }}
        >
          <Comment />
          Send feedback…
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => openSettings("shortcuts")}>
          <Keyboard />
          Keyboard shortcuts
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
