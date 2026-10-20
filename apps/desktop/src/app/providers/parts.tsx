import type { ComponentProps, ReactNode } from "react";

import { SettingsButton, SettingsSection } from "@/app/settings/parts";
import { Badge } from "@/components/ui/badge";
import { ProviderGlyph, Reload } from "@/components/icons";
import type { ProviderKind, QuotaWindow } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { usageTone, used, windowTitle } from "@/lib/quota";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Providers is built from, laid out as the user's design has it and drawn
 * with every Settings page's parts: a section (its title and a note on the right over a card),
 * cards, plan chips, an agent's mark and a usage meter.
 */

/** A section: its title and an optional note on the right, over its card. */
export function ProvidersSection({
  title,
  aside,
  setting,
  children,
}: {
  title: string;
  aside?: ReactNode;
  /** The name Settings search finds the section by, when not its title. */
  setting?: string;
  children: ReactNode;
}) {
  return (
    <div data-setting={setting ?? title}>
      <SettingsSection
        title={title}
        actions={aside && <div className="text-foreground/65 text-label flex items-center gap-1">{aside}</div>}
      >
        {children}
      </SettingsSection>
    </div>
  );
}

/** A row's text: its title over its line, each in the Settings rows' type. */
export const ROW_TITLE = "text-label font-medium";
export const ROW_LINE = "text-foreground/65 text-xs";

/** Check again, at the top of the page and of an agent's page. */
export function CheckAgainButton({ busy, ...props }: ComponentProps<"button"> & { busy: boolean }) {
  return (
    <SettingsButton disabled={busy} {...props}>
      <Reload aria-hidden className={busy ? "animate-spin motion-reduce:animate-none" : undefined} />
      Check again
    </SettingsButton>
  );
}

/** "Checked 3m ago" with its dot. */
export function CheckedNote({ children }: { children: ReactNode }) {
  return (
    <span className="text-muted-foreground text-label flex items-center gap-1.5">
      <span aria-hidden className="bg-success size-1.5 rounded-full" />
      {children}
    </span>
  );
}

/** An agent's plan ("Max"), beside its name; `large` beside an agent page's title. */
export function PlanChip({ plan, large = false }: { plan: string; large?: boolean }) {
  return (
    <Badge data-slot="plan-chip" variant="secondary" className={cn("shrink-0", !large && "text-2xs font-normal")}>
      {plan.charAt(0).toUpperCase()}
      {plan.slice(1)}
    </Badge>
  );
}

/**
 * An agent's mark in its rounded square; `idle` for one not set up, `dim` for one off, `large`
 * beside an agent page's title.
 */
export function AgentMark({
  provider,
  idle = false,
  dim = false,
  large = false,
}: {
  provider: ProviderKind;
  idle?: boolean;
  dim?: boolean;
  large?: boolean;
}) {
  return (
    <span
      aria-hidden
      data-slot="agent-mark"
      data-provider={provider}
      className={cn(
        "rounded-nav flex shrink-0 items-center justify-center",
        large ? "size-11" : "size-8.5",
        idle ? "border-divider border" : "bg-foreground/8",
        dim && "opacity-50",
      )}
    >
      <ProviderGlyph
        provider={provider}
        className={cn(
          large ? "size-5.5" : "size-4.25",
          provider === "codex" && (idle ? "text-muted-foreground" : "text-foreground/85"),
        )}
      />
    </span>
  );
}

/** The dot before an agent's state: ready, needing a step, or off. */
export function StateDot({ tone }: { tone: "ready" | "needs" | "off" }) {
  return (
    <span
      aria-hidden
      className={cn(
        "size-1.5 shrink-0 rounded-full",
        tone === "ready" ? "bg-success" : tone === "needs" ? "bg-warning" : "bg-muted-foreground",
      )}
    />
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
          {percent}%
        </span>
      </span>
      <span aria-hidden className="bg-muted rounded-capsule h-1 overflow-hidden">
        <span className={cn("rounded-capsule block h-full", colors.fill)} style={{ width: `${percent}%` }} />
      </span>
      {!compact && resets && (
        <span className="text-muted-foreground text-2xs">Resets in {resets}</span>
      )}
    </div>
  );
}
