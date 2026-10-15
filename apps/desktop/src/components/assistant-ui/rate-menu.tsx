import {
  ThumbDown,
  ThumbDownFilled,
  Thumbs,
  ThumbUp,
  ThumbUpFilled,
} from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Rating } from "@/ipc/generated";

/**
 * "Feedback" under an answer: one button that opens "Good response" and "Bad response", and
 * shows the thumb the user picked. The items are `RateItem`s, bound
 * to wherever the rating goes.
 */
export function RateMenu({ rated, children }: { rated: Rating | null; children: ReactNode }) {
  const Icon = rated === "good" ? ThumbUpFilled : rated === "bad" ? ThumbDownFilled : Thumbs;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton tooltip="Feedback">
          <Icon />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start">
        {children}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** "Good response" or "Bad response" in the rate menu. */
export function RateItem({
  rating,
  ...props
}: { rating: Rating } & ComponentProps<typeof DropdownMenuItem>) {
  return (
    <DropdownMenuItem {...props}>
      {rating === "good" ? <ThumbUp /> : <ThumbDown />}
      {rating === "good" ? "Good response" : "Bad response"}
    </DropdownMenuItem>
  );
}
