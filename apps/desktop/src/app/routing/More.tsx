import type { ReactNode } from "react";

import { refreshRunning } from "@/app/routing/rankings";
import { Badge, Card, Glyph, HAIRLINE_BOTTOM, HAIRLINE_TOP, LinkButton, ListOrderedIcon, OpenChevron, PillSelect, RowText, Section, StepNumber } from "@/app/routing/parts";
import { faded, reviewers, reviewsWords, ROUTED_KINDS, strengthFor, tierFor } from "@/app/routing/words";
import { ArrowRotateCw, SpeedometerLatencySpeed } from "@/components/ui/icon";
import type { RoutePreview, TaskCategory } from "@/ipc/generated";
import { PLAIN_KIND_NAMES, TIER_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { modelAvailable } from "@/state/providers";
import type { RankingsRefreshView } from "@/state/rankings";
import { useApp } from "@/state/store";
import { useUsage } from "@/state/usage";

/** More's rows, for the page and Settings search. */
export const MORE_ROWS = {
  strength: { label: "Model strength", description: "How good each model is at a kind of work, out of 10" },
  steps: {
    label: "How a model is picked",
    description: "Limits, then your rules, tier floor, your order and finally the best score",
  },
  refresh: {
    label: "Refresh ratings",
    description:
      "Looks up the latest on each model on the web with one of your models, then updates how Brigadier rates them. Your own choices stay as they are.",
  },
} as const;

/** The steps routing narrows the models by, in order. */
const STEPS: readonly [string, string][] = [
  ["Hard limits", "Skips models that are signed out, at a limit, just failed or can't read images"],
  ["Never rules", "Skips models your rules keep away from this work"],
  ["Tier floor", "Skips models below the tier this work needs, e.g. Frontier for code, review and merge"],
  ["Your order", "If you set one, the first model that can run takes it. Skips the next two steps"],
  ["Prefer rules", "Moves models your rules prefer to the front"],
  ["Orchestrator pin", "Coordinate work stays with the model that started it"],
  ["Best score", "Otherwise the highest of strength + learned − quota − load"],
];

/** Which of More's rows is open. */
export type MoreRow = "strength" | "steps" | null;

/** "3 hours ago": `n` of a unit ago. */
const ago = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;

/** "3 hours ago": how long since a check. */
function since(ms: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - ms) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return ago(minutes, "minute");
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? ago(hours, "hour") : ago(Math.floor(hours / 24), "day");
}

/** "Oct 13": the ratings' date (a calendar day, so read in UTC). */
function day(date: string): string {
  const parsed = new Date(date);
  return Number.isNaN(parsed.getTime())
    ? date
    : parsed.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}

/**
 * More: Model strength (the models ranked for one kind of work, with Refresh ratings) and How a
 * model is picked, each opening in place.
 */
export function More({
  open,
  onOpen,
  category,
  onCategory,
  route,
  rankings,
  now,
}: {
  open: MoreRow;
  onOpen: (row: MoreRow) => void;
  /** The kind of work the models are ranked for. */
  category: TaskCategory;
  onCategory: (category: TaskCategory) => void;
  /** What routing picks for that kind now, for Picked now. */
  route: RoutePreview | null;
  rankings: RankingsRefreshView;
  now: number;
}) {
  const view = useUsage((s) => s.view);
  const availability = useApp((s) => s.settings);
  // Models switched off on Providers (or whose agent is) get no work, so they aren't ranked here.
  const ranked = (view?.models ?? [])
    .filter((model) => !model.excluded && modelAvailable(availability, { provider: model.provider, id: model.id }))
    .flatMap((model) => {
      const strength = strengthFor(model, category);
      return strength === null ? [] : [{ model, strength }];
    })
    .toSorted((a, b) => b.strength - a.strength);
  const picked = route?.outcome.type === "chosen" ? route.outcome.choice : null;
  // A review picked for each vendor's change marks whose changes each model reviews.
  const byAuthor = reviewers(route);
  const registry = view?.registry ?? null;
  const { refresh, starting, start, error } = rankings;
  const running = refresh !== null && refreshRunning(refresh.state);
  // A refresh admitted but failing later says why, under the footer.
  const failed =
    refresh?.state === "failed"
      ? `The refresh failed, so the ratings didn't change${refresh.errors.length > 0 ? `: ${refresh.errors.join("; ")}` : "."}`
      : null;
  const failure = starting.error ?? error ?? failed;
  const toggle = (row: Exclude<MoreRow, null>) => onOpen(open === row ? null : row);
  return (
    <Section title="More">
      <Card data-slot="routing-more">
        <MoreRowHead
          row={MORE_ROWS.strength}
          icon={<SpeedometerLatencySpeed aria-hidden />}
          open={open === "strength"}
          summary={ranked[0] && `${ranked[0].model.displayName} leads`}
          onToggle={() => toggle("strength")}
        >
          <div data-slot="model-strength" className="flex flex-col ps-7.5">
            <div className="border-divider flex items-center gap-6 border-y py-3.5">
              <RowText quiet title="Kind of work" description="Which work to rank the models for" />
              <PillSelect
                label="The kind of work the models are ranked for"
                value={category}
                options={ROUTED_KINDS.map((kind) => ({ value: kind, label: PLAIN_KIND_NAMES[kind] }))}
                onChange={onCategory}
                align="end"
              />
            </div>
            {!view && <p className="text-xs text-muted-foreground py-3">Reading the models…</p>}
            {ranked.map(({ model, strength }, index) => {
              const dim = faded(model, category);
              const isPicked = picked?.provider === model.provider && picked.model === model.id;
              return (
                <div
                  key={`${model.provider}/${model.id}`}
                  data-slot="strength-row"
                  className={cn(HAIRLINE_BOTTOM, "flex items-center gap-3.5 py-3")}
                >
                  <span className="text-xs text-muted-foreground w-3.5 shrink-0 font-mono">{index + 1}</span>
                  <span className={cn("flex min-w-0 flex-1 items-center gap-2.5", dim && "opacity-50")}>
                    <Glyph provider={model.provider} className="size-3.75" />
                    <span className="flex min-w-0 items-center gap-2">
                      <span data-slot="strength-name" className="text-label text-foreground truncate font-medium">
                        {model.displayName}
                      </span>
                      <span className="text-xs text-muted-foreground shrink-0">
                        {TIER_LABELS[tierFor(model, category)]}
                      </span>
                    </span>
                  </span>
                  <span aria-hidden className={cn("bg-muted h-1 w-105 shrink rounded-full", dim && "opacity-50")}>
                    <span
                      className={cn("block h-full rounded-full", isPicked ? "bg-foreground" : "bg-muted-foreground/60")}
                      style={{ width: `${Math.max(0, Math.min(10, strength)) * 10}%` }}
                    />
                  </span>
                  <span
                    className={cn(
                      "text-xs w-7.5 shrink-0 text-end font-mono font-medium",
                      dim ? "text-muted-foreground" : "text-foreground",
                    )}
                  >
                    {strength.toFixed(1)}
                  </span>
                  <span className="flex min-w-23 shrink-0 justify-end">
                    {byAuthor ? (
                      byAuthor.has(`${model.provider}/${model.id}`) && (
                        <Badge tone="success">{reviewsWords(byAuthor.get(`${model.provider}/${model.id}`)!)}</Badge>
                      )
                    ) : (
                      isPicked && <Badge tone="success">Picked now</Badge>
                    )}
                  </span>
                </div>
              );
            })}
            <div className="flex items-center gap-3.5 py-3">
              <span className="text-xs text-muted-foreground min-w-0 flex-1">
                {registry ? `Ratings updated ${day(registry.updated)}` : "Ratings"}
                {registry?.checkedAtMs != null && ` · checked ${since(registry.checkedAtMs, now)}`}
              </span>
              <LinkButton
                data-setting={MORE_ROWS.refresh.label}
                className="text-xs gap-1.5 font-medium"
                disabled={running || starting.busy || refresh === null}
                onClick={() => starting.run(start)}
              >
                <ArrowRotateCw
                  aria-hidden
                  className={cn("size-3", (running || starting.busy) && "animate-spin motion-reduce:animate-none")}
                />
                {running ? "Refreshing…" : MORE_ROWS.refresh.label}
              </LinkButton>
            </div>
            {failure && (
              <p role="alert" className="text-destructive text-xs pb-3">
                {failure}
              </p>
            )}
          </div>
        </MoreRowHead>
        <MoreRowHead
          row={MORE_ROWS.steps}
          icon={<ListOrderedIcon aria-hidden />}
          open={open === "steps"}
          onToggle={() => toggle("steps")}
        >
          <div data-slot="routing-steps" className="ps-7.5 pb-1">
            <ol className={cn(HAIRLINE_TOP, "flex flex-col pt-1.5 pb-2.5")}>
              {STEPS.map(([step, hint], index) => (
                <li key={step} className="flex gap-3.5 py-2.5">
                  <StepNumber>{index + 1}</StepNumber>
                  <span className="flex min-w-0 flex-1 flex-col gap-0.75 pt-0.5">
                    <span className="text-label text-foreground">{step}</span>
                    <span className="text-xs text-foreground/65">{hint}</span>
                  </span>
                </li>
              ))}
            </ol>
          </div>
        </MoreRowHead>
      </Card>
    </Section>
  );
}

/** One of More's rows: its icon, name and line, a summary, and what it opens to. */
function MoreRowHead({
  row,
  icon,
  open,
  summary,
  onToggle,
  children,
}: {
  row: { label: string; description: string };
  icon: ReactNode;
  open: boolean;
  summary?: string | undefined;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <div data-setting={row.label} data-open={open || undefined} className="flex flex-col">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="flex w-full items-center gap-3.5 py-3.5 text-start [&>svg:first-child]:text-foreground/65 [&>svg:first-child]:size-4 [&>svg:first-child]:shrink-0"
      >
        {icon}
        <RowText title={row.label} description={row.description} />
        {summary && !open && <span className="text-xs text-muted-foreground shrink-0">{summary}</span>}
        <OpenChevron open={open} />
      </button>
      {open && children}
    </div>
  );
}
