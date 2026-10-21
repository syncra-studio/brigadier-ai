import type { ComponentProps, ReactNode } from "react";

import { Lightbulb } from "@/components/icons";
import { cn } from "@/lib/utils";

/** The card's frame: its header ("Plan" or "Writing plan", with a bulb) and what it holds. */
export function PlanFrame({
  label,
  live = false,
  actions,
  children,
  ...props
}: {
  label: string;
  live?: boolean;
  actions?: ReactNode;
  children?: ReactNode;
} & Omit<ComponentProps<"section">, "children">) {
  return (
    <section
      data-slot="plan-card"
      className="border-work-rule bg-foreground/3 rounded-plan flex min-w-0 flex-col border"
      {...props}
    >
      <header className="h-plan-header flex shrink-0 items-center gap-2 ps-3.5 pe-2">
        <Lightbulb aria-hidden className="text-foreground/50 size-icon-md shrink-0" />
        <span className={cn("min-w-0 flex-1 truncate text-sm", live ? "shimmer" : "text-foreground/50")}>
          {label}
        </span>
        {actions && <span className="flex shrink-0 items-center gap-0.5">{actions}</span>}
      </header>
      {children}
    </section>
  );
}
