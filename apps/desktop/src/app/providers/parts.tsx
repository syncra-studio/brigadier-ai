import { Reload } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import type { ProviderKind, QuotaWindow } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { usageLevel, used, windowTitle } from "@/lib/quota";
import { cn } from "@/lib/utils";

/*
 * The parts Settings › Providers is built from, drawn to the user's own design: a section (its
 * title and a note on the right over a card), cards, pill buttons, plan chips, an agent's mark
 * and a usage meter. Its colours and type are the redesigned Settings pages' (`settings-*`).
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
    <section data-setting={setting ?? title} aria-label={title} className="flex flex-col gap-3.5">
      <div className="flex items-center justify-between gap-6">
        <h2 className="text-settings-text text-settings-heading font-medium">{title}</h2>
        {aside && (
          <div className="text-settings-text-tertiary flex items-center gap-1 text-settings-small">
            {aside}
          </div>
        )}
      </div>
      {children}
    </section>
  );
}

/**
 * A card of rows split by hairlines: `rows` keeps its rows' text 20 in from its edges, `list`
 * (the agents) 8, its rows padding their own.
 */
export function ProvidersCard({
  inset = "rows",
  className,
  children,
}: {
  inset?: "rows" | "list";
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      data-slot="providers-card"
      className={cn(
        "bg-settings-surface border-settings-border rounded-providers-card divide-settings-divider flex flex-col divide-y border",
        inset === "rows" ? "px-5" : "px-2",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** A small pill button on a row: Use, Update, Change, Sign in. */
export function ProvidersButton({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "bg-settings-raised border-settings-ring text-settings-text hover:bg-settings-border rounded-capsule flex h-7 shrink-0 items-center border px-3 text-settings-small font-medium transition-colors disabled:opacity-50",
        className,
      )}
    />
  );
}

/** The light pill that starts setting an agent up. */
export function SetUpButton({ className, ...props }: ComponentProps<"button">) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "bg-settings-text text-settings-on-light rounded-capsule flex h-7.5 shrink-0 items-center px-3.5 text-settings-small font-medium transition-opacity hover:opacity-90 disabled:opacity-50",
        className,
      )}
    />
  );
}

/** Check again, at the top of the page and of an agent's page. */
export function CheckAgainButton({ busy, ...props }: ComponentProps<"button"> & { busy: boolean }) {
  return (
    <button
      type="button"
      disabled={busy}
      {...props}
      className="bg-settings-surface border-settings-border text-settings-text hover:bg-settings-raised rounded-capsule flex h-8 shrink-0 items-center gap-1.75 border px-3 text-settings-small transition-colors disabled:opacity-60"
    >
      <Reload aria-hidden className={cn("size-3.25", busy && "animate-spin motion-reduce:animate-none")} />
      Check again
    </button>
  );
}

/** "Checked 3m ago" with its dot. */
export function CheckedNote({ children }: { children: ReactNode }) {
  return (
    <span className="text-settings-text-secondary flex items-center gap-1.75 text-settings-small">
      <span aria-hidden className="bg-settings-success size-1.5 rounded-full" />
      {children}
    </span>
  );
}

/** An agent's plan ("Max"), beside its name; `large` beside an agent page's title. */
export function PlanChip({ plan, large = false }: { plan: string; large?: boolean }) {
  return (
    <span
      data-slot="plan-chip"
      className={cn(
        "bg-settings-border shrink-0 font-medium",
        large
          ? "text-providers-chip-text-strong rounded-providers-chip-lg px-2 py-0.75 text-xs"
          : "text-providers-chip-text rounded-providers-chip px-1.75 py-0.5 text-2xs",
      )}
    >
      {plan.charAt(0).toUpperCase()}
      {plan.slice(1)}
    </span>
  );
}

/** An agent's mark in its rounded square; `idle` for one not set up, `dim` for one off. */
export function AgentMark({
  provider,
  idle = false,
  dim = false,
}: {
  provider: ProviderKind;
  idle?: boolean;
  dim?: boolean;
}) {
  return (
    <span
      aria-hidden
      className={cn(
        "rounded-providers-mark flex size-8.5 shrink-0 items-center justify-center",
        idle ? "bg-settings-raised border-settings-control-border border" : "bg-settings-chosen",
        dim && "opacity-50",
      )}
    >
      <ProviderGlyph
        provider={provider}
        className={cn(
          "size-4.25",
          provider === "codex" && (idle ? "text-settings-text-secondary" : "text-settings-glyph"),
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
        tone === "ready" ? "bg-settings-success" : tone === "needs" ? "bg-settings-warning" : "bg-settings-text-tertiary",
      )}
    />
  );
}

/** A usage share's colours: the design's grey bar, its amber near the limit, red at it. */
export function usageColors(percent: number): { text: string; fill: string } {
  const level = usageLevel(percent);
  if (level === "danger") return { text: "text-destructive", fill: "bg-destructive" };
  if (level === "warning") return { text: "text-settings-warning", fill: "bg-settings-warning" };
  return { text: "text-settings-text", fill: "bg-providers-bar" };
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
  const colors = usageColors(percent);
  const resets = window.resetsAtMs !== null ? formatCountdown(window.resetsAtMs, now) : null;
  return (
    <div
      data-slot="account-usage"
      className={cn("flex min-w-0 flex-col", compact ? "w-37.5 gap-1.5" : "w-35 gap-1.75")}
      title={`${window.label}: ${percent}% used${resets ? `, resets in ${resets}` : ""}`}
    >
      <span className="flex items-baseline justify-between gap-2 text-settings-meta whitespace-nowrap">
        <span className={cn("truncate", compact ? "text-settings-text-tertiary" : "text-settings-text-secondary")}>
          {label ?? windowTitle(window)}
        </span>
        <span data-slot="usage-value" className={cn("font-medium", colors.text)}>
          {percent}%
        </span>
      </span>
      <span aria-hidden className="bg-settings-track rounded-providers-bar h-1 overflow-hidden">
        <span className={cn("rounded-providers-bar block h-full", colors.fill)} style={{ width: `${percent}%` }} />
      </span>
      {!compact && resets && (
        <span className="text-settings-text-tertiary text-settings-badge">Resets in {resets}</span>
      )}
    </div>
  );
}
