import type { FC, HTMLAttributes, MouseEvent, ReactNode } from "react";

import { OPENS } from "@/components/conversation/elements/activity-row";
import { Collapsible, CollapsibleContent } from "@/components/layout/collapsible";
import { cn } from "@/lib/utils";

/** What a click in a card's header leaves to itself: its buttons, links and fields. */
const OWN_ACTION = "button, a, input, textarea, select, [role=menu], [role=menuitem]";

/**
 * A folding card's header row (the Changes and Preview cards): a click anywhere on it folds or
 * opens the card, as its chevron does, except on a button or link inside it, which keeps its own
 * action, or a click that ends a text selection. The chevron stays the control keys and screen
 * readers use.
 */
export const FoldHeader: FC<HTMLAttributes<HTMLDivElement> & { foldable: boolean; onToggle: () => void }> = ({
  foldable,
  onToggle,
  className,
  onClick,
  ...props
}) => (
  // oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions -- the chevron is the keyboard's fold control
  <div
    data-slot="card-fold-header"
    {...props}
    className={cn(className, foldable && "cursor-pointer")}
    onClick={(event: MouseEvent<HTMLDivElement>) => {
      onClick?.(event);
      const target = event.target as Element;
      // A menu the header opened lives in a portal: its clicks reach here through React only.
      if (!foldable || !event.currentTarget.contains(target) || target.closest(OWN_ACTION)) return;
      if (window.getSelection()?.toString()) return;
      onToggle();
    }}
  />
);

/**
 * A folding card's body: its height and opacity open and close as the thread's rows do, at once
 * with reduced motion.
 */
export const FoldBody: FC<{ open: boolean; children: ReactNode }> = ({ open, children }) => (
  <Collapsible open={open}>
    <CollapsibleContent data-slot="card-fold-body" className={OPENS}>
      {children}
    </CollapsibleContent>
  </Collapsible>
);
