import type { ComponentProps, ReactNode } from "react";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  ThumbDown,
  ThumbDownFilled,
  Thumbs,
  ThumbUp,
  ThumbUpFilled,
  Undo,
} from "@/components/ui/icon";
import type { Rating } from "@/ipc/generated";

/**
 * "Feedback" under an answer (or "Rate this plan" on a plan): one button that opens "Good
 * response" and "Bad response", and shows the thumb the user picked; once rated, "Clear rating"
 * takes it back (#127). The items are `RateItem`s, bound to wherever the rating goes.
 */
export function RateMenu({ rated, children, onClear, tooltip = "Feedback", below = false, className }: {
  rated: Rating | null;
  children: ReactNode;
  onClear: () => void;
  tooltip?: string;
  /** It opens down and to its left, as at the end of a card's title row. */
  below?: boolean;
  className?: string;
}) {
  const Icon = rated === "good" ? ThumbUpFilled : rated === "bad" ? ThumbDownFilled : Thumbs;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton tooltip={tooltip} data-slot="rate-menu" data-rated={rated ?? undefined} className={className}>
          <Icon />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent side={below ? "bottom" : "top"} align={below ? "end" : "start"}>
        {children}
        {rated && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={onClear}>
              <Undo />
              Clear rating
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** "Good response" or "Bad response" in the rate menu ("Good plan" or "Bad plan" for a plan). */
export function RateItem({
  rating,
  of = "response",
  ...props
}: { rating: Rating; of?: "response" | "plan" } & ComponentProps<typeof DropdownMenuItem>) {
  return (
    <DropdownMenuItem {...props}>
      {rating === "good" ? <ThumbUp /> : <ThumbDown />}
      {`${rating === "good" ? "Good" : "Bad"} ${of}`}
    </DropdownMenuItem>
  );
}
