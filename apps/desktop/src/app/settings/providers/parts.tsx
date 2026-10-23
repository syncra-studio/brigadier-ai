import type { ComponentProps } from "react";

import { Badge, Button } from "@/components/controls";
import { Reload } from "@/components/icons";
import { UsageBar } from "@/components/shared";
import type { QuotaWindow } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { usageTone, used, windowTitle } from "@/lib/quota";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Providers is built from, laid out as the user's design has it and drawn
 * with every Settings page's parts: a section (its title and a note on the right over a card),
 * cards, plan chips, an agent's mark and a usage meter.
 */

/** A row's text: its title over its line, each in the Settings rows' type. */
export const ROW_TITLE = "text-label font-medium";
export const ROW_LINE = "text-foreground/65 text-xs";

/** Check again, at the top of the page and of an agent's page. */
export function CheckAgainButton({ busy, ...props }: ComponentProps<"button"> & { busy: boolean }) {
  return (
    <Button variant="soft" size="sm" disabled={busy} {...props}>
      <Reload aria-hidden className={busy ? "animate-spin motion-reduce:animate-none" : undefined} />
      Check again
    </Button>
  );
}

/** An agent's plan ("Max"), beside its name; `large` beside an agent page's title. */
export function PlanChip({ plan, large = false }: { plan: string; large?: boolean }) {
  return (
    <Badge data-slot="plan-chip" variant="secondary" className={cn("shrink-0", !large && "text-2xs font-normal")}>
      {plan.charAt(0).toUpperCase() + plan.slice(1)}
    </Badge>
  );
}

/**
 * A usage window: its name and share over a thin bar, and when it resets. `label` replaces its
 * name ("Weekly · in use"); `compact` is the agents list's (no reset line, a 6 gap).
 */
export function UsageMeter({
  window,
  now,
  label,
  compact = false,
}: {
  window: QuotaWindow;
  now: number;
  label?: string;
  compact?: boolean;
}) {
  const percent = used(window);
  const colors = usageTone(percent);
  const resets = window.resetsAtMs !== null ? formatCountdown(window.resetsAtMs, now) : null;
  return (
    <div
      data-slot="account-usage"
      className={cn("flex min-w-0 flex-col", compact ? "w-37.5 gap-1.5" : "w-35 gap-1.75")}
      title={`${window.label}: ${percent}% used${resets ? `, resets in ${resets}` : ""}`}
    >
      <span className="flex items-baseline justify-between gap-2 text-xs whitespace-nowrap tabular-nums">
        <span className={cn("truncate", compact ? "text-muted-foreground" : "text-foreground/65")}>
          {label ?? windowTitle(window)}
        </span>
        <span data-slot="usage-value" className={cn("font-medium", colors.text)}>
          {percent}% used
        </span>
      </span>
      <UsageBar value={percent} fill={colors.fill} />
      {!compact && resets && (
        <span className="text-muted-foreground text-2xs">Resets in {resets}</span>
      )}
    </div>
  );
}
