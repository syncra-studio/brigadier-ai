import type { ComponentProps, FC } from "react";

import { ChevronDown, ChevronLeft, ChevronRight, ChevronUp } from "@/components/icons";
import { cn } from "@/lib/utils";

const ARROW = "enabled:hover:text-foreground rounded-control p-0.5 disabled:opacity-40 [&_svg]:size-icon-xs";

/**
 * "‹ 1 of 3 ›": bare arrows, back and on (`vertical`: up and down), around "n of m" when `at` and
 * `count` are given ("n of m Files" with a `unit`). `noun` names the arrows ("Previous change").
 */
export const StepCounter: FC<
  Omit<ComponentProps<"span">, "children"> & {
    noun: string;
    at?: number;
    count?: number;
    unit?: string;
    onStep: (step: -1 | 1) => void;
    canBack?: boolean;
    canForward?: boolean;
    vertical?: boolean;
  }
> = ({ noun, at, count, unit, onStep, canBack = true, canForward = true, vertical = false, className, ...props }) => {
  const [Back, On] = vertical ? [ChevronUp, ChevronDown] : [ChevronLeft, ChevronRight];
  return (
    <span {...props} className={cn("text-muted-foreground flex shrink-0 items-center gap-0.5 text-xs tabular-nums", className)}>
      <button type="button" aria-label={`Previous ${noun}`} className={ARROW} disabled={!canBack} onClick={() => onStep(-1)}>
        <Back />
      </button>
      {at !== undefined && count !== undefined && (
        <span className="whitespace-nowrap">
          {at + 1} of {count}
          {unit && ` ${unit}`}
        </span>
      )}
      <button type="button" aria-label={`Next ${noun}`} className={ARROW} disabled={!canForward} onClick={() => onStep(1)}>
        <On />
      </button>
    </span>
  );
};
