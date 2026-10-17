import { ChevronDown, ChevronRight, Flask, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { Fragment, type ReactNode, useState } from "react";

import { SourceLink } from "@/app/routing/models";
import {
  changedModels,
  fieldWords,
  overlayStatus,
  refreshRunning,
  refreshStatus,
  type RefreshStatus,
} from "@/app/routing/rankings";
import { effortRange, formatPart, KIND_HEADS, KIND_ICONS, loadFactor, quotaFactor, ROUTED_KINDS, spacing } from "@/app/routing/words";
import { SettingsButton } from "@/app/settings/parts";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type {
  Area,
  Learned,
  MergedModel,
  QualityTier,
  RankingsRefresh,
  RegistryInfo,
  RouteCandidate,
  TaskCategory,
} from "@/ipc/generated";
import { formatAgo, formatTokens } from "@/lib/format";
import {
  AREA_LABELS,
  AREAS,
  DEFAULT_FLOORS,
  HEAT_LABELS,
  joinWords,
  PROVIDERS,
  TIER_LABELS,
  tierBelow,
  VENDOR_LABELS,
} from "@/lib/routing";
import { cn } from "@/lib/utils";
import { modelAvailable } from "@/state/providers";
import type { RankingsRefreshView } from "@/state/rankings";
import { useApp } from "@/state/store";

/** The ratings card's copy, for the page and Settings search. */
export const RATINGS_ROWS = {
  ratings: {
    label: "Model ratings",
    description:
      "Strength from 0 to 10 for each kind of work, weighing quality against cost. Faded scores sit below that kind's tier floor, so Automatic won't pick them.",
  },
  refresh: {
    label: "Refresh rankings",
    description:
      "Looks up the latest on each model on the web with one of your models, then updates how Brigadier rates them. Your own choices stay as they are.",
  },
} as const;

/** What a learned adjustment can reach either way, and what an area can add or take. */
const MAX_LEARNED = 2.5;
const MAX_AREA = 2;
/** What a legacy model's score loses, and an inherited one's until it is proven. */
const LEGACY_PENALTY = 1;
const INHERITED_PENALTY = 0.25;

/** Shorter tier names for the floor row. */
const FLOOR_WORDS: Record<QualityTier, string> = {
  unrated: "Any",
  light: "Light",
  standard: "Std",
  strong: "Strong",
  frontier: "Frontier",
};

const STATUS_TONES: Record<RefreshStatus["tone"], string> = {
  running: "text-foreground/80",
  done: "text-foreground/65",
  warning: "text-warning",
  error: "text-destructive",
};

/** The table's columns: model, tier, the ten kinds of work, context, effort and rating. */
const COLUMNS = {
  gridTemplateColumns: `minmax(${spacing(28)},1fr) ${spacing(22)} repeat(10,${spacing(14)}) ${spacing(14)} ${spacing(18)} ${spacing(28)}`,
};

/** The tier a model counts as for a kind of work (a model of some kinds only has its own per kind). */
function tierFor(model: MergedModel, category: TaskCategory): QualityTier {
  return model.only?.[category] ?? model.tier;
}

/** A model's score for a kind of work, when it takes that kind. */
function strengthFor(model: MergedModel, category: TaskCategory): number | null {
  if (model.only && !(category in model.only)) return null;
  return model.strengths[category] ?? null;
}

/** Whether a score is shown faded: below the kind's floor, or a legacy model's. */
function faded(model: MergedModel, category: TaskCategory): boolean {
  return model.legacy || tierBelow(tierFor(model, category), DEFAULT_FLOORS[category]);
}

/** Where a model's rating comes from, in a few words, and its tone. */
function ratingWords(model: MergedModel): { text: string; tone: string } {
  if (model.only) {
    const kinds = Object.keys(model.only) as TaskCategory[];
    return { text: `${joinWords(kinds.map((kind) => KIND_HEADS[kind]))} only`, tone: "text-muted-foreground" };
  }
  if (model.legacy) return { text: `Legacy · −${LEGACY_PENALTY}`, tone: "text-warning" };
  if (model.status === "inherited") {
    return {
      text: model.trial ? `Inherited · trial ${model.trial.outcomes}/${model.trial.needed}` : "Inherited",
      tone: "text-link",
    };
  }
  if (model.ratingProvenance === "researchedOverlay" || model.status === "researched") {
    return { text: "Researched", tone: "text-muted-foreground" };
  }
  if (model.status === "unknown") {
    return { text: model.trial ? `Unrated · trial ${model.trial.outcomes}/${model.trial.needed}` : "Unrated", tone: "text-link" };
  }
  return { text: "Registry", tone: "text-muted-foreground" };
}

/** The best score shown unfaded per kind of work, across every model. */
function bestScores(models: readonly MergedModel[]): Map<TaskCategory, number> {
  const best = new Map<TaskCategory, number>();
  for (const category of ROUTED_KINDS) {
    for (const model of models) {
      const strength = strengthFor(model, category);
      if (strength === null || faded(model, category)) continue;
      if (strength > (best.get(category) ?? -1)) best.set(category, strength);
    }
  }
  return best;
}

/**
 * Model ratings: every model routing may give work to, with its strength for each kind of work,
 * its tier against each kind's floor, its context and efforts, and where its rating comes from;
 * a row opens its profile and what it learned in the project scored for. Refresh rankings
 * researches the models again.
 */
export function ModelRatings({
  models,
  learned,
  registry,
  rankings,
  category,
  projectName,
  candidates,
  now,
}: {
  models: readonly MergedModel[] | null;
  learned: readonly Learned[];
  registry: RegistryInfo | null;
  rankings: RankingsRefreshView;
  /** The kind of work the page shows: a profile's learned number is for it. */
  category: TaskCategory;
  /** The project the learned numbers are for; `null`: every project. */
  projectName: string | null;
  /** The ranking's candidates, for each model's quota and load now. */
  candidates: readonly RouteCandidate[];
  now: number;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const availability = useApp((s) => s.settings);
  // Models switched off on Providers (or whose agent is) get no work, so they aren't rated here.
  const routed = (models ?? []).filter(
    (model) => !model.excluded && modelAvailable(availability, { provider: model.provider, id: model.id }),
  );
  const curated = routed.filter((model) => model.status === "curated");
  const others = routed.filter((model) => model.status !== "curated");
  const best = bestScores(routed);
  const rows = (list: readonly MergedModel[]) =>
    list.map((model) => {
      const key = `${model.provider}/${model.id}`;
      return (
        <RatingRow
          key={key}
          model={model}
          best={best}
          open={open === key}
          onToggle={() => setOpen(open === key ? null : key)}
          learned={learned}
          registry={registry}
          category={category}
          projectName={projectName}
          candidate={candidates.find((found) => found.provider === model.provider && found.model === model.id) ?? null}
        />
      );
    });

  return (
    <section
      data-setting={RATINGS_ROWS.ratings.label}
      data-slot="model-ratings"
      className="bg-card border-divider rounded-settings flex flex-col overflow-hidden border"
    >
      <div className="flex items-start justify-between gap-6 px-6 pt-5 pb-4">
        <div className="flex max-w-lg min-w-0 flex-1 flex-col gap-1">
          <h2 className="text-lg font-medium">{RATINGS_ROWS.ratings.label}</h2>
          <p className="text-foreground/65 text-xs">{RATINGS_ROWS.ratings.description}</p>
        </div>
        <div className="flex shrink-0 flex-wrap items-center justify-end gap-3 text-xs">
          {registry && (
            <span
              className="border-divider text-muted-foreground rounded-xs border px-1.5 py-0.5 font-mono"
              title={registry.source === "bundled" ? "Bundled with this version of Brigadier" : "Downloaded from GitHub"}
            >
              registry r{registry.revision} · {registry.updated}
            </span>
          )}
          {registry?.checkedAtMs != null && (
            <span className="text-muted-foreground flex items-center gap-1.5">
              <span aria-hidden className={cn("size-1.5 rounded-full", registry.error ? "bg-warning" : "bg-success")} />
              Checked GitHub {formatAgo(registry.checkedAtMs, now)}
            </span>
          )}
          <RefreshButton rankings={rankings} />
        </div>
      </div>
      <RefreshLines rankings={rankings} models={models} registry={registry} />
      <div className="overflow-x-auto text-xs">
        <div style={{ minWidth: spacing(280) }}>
          <div className="text-muted-foreground border-divider grid items-center gap-1 border-y px-6 py-2" style={COLUMNS}>
            <span>Model</span>
            <span>Tier</span>
            {ROUTED_KINDS.map((kind) => {
              const Icon = KIND_ICONS[kind];
              return (
                <span key={kind} className="flex min-w-0 flex-col items-center gap-0.5 text-2xs">
                  <Icon aria-hidden className="size-icon-xs" />
                  {KIND_HEADS[kind]}
                </span>
              );
            })}
            <span className="text-end">Context</span>
            <span className="text-end">Effort</span>
            <span className="text-end">Rating</span>
          </div>
          <div className="text-muted-foreground border-divider grid items-center gap-1 border-b px-6 py-1.5 text-2xs" style={COLUMNS}>
            <span>Tier floor</span>
            <span />
            {ROUTED_KINDS.map((kind) => (
              <span
                key={kind}
                className={cn("text-center", DEFAULT_FLOORS[kind] === "frontier" && "text-foreground font-medium")}
              >
                {FLOOR_WORDS[DEFAULT_FLOORS[kind]]}
              </span>
            ))}
            <span />
            <span />
            <span />
          </div>
          {!models && <p className="text-muted-foreground px-6 py-3">Reading the models…</p>}
          {PROVIDERS.map((provider) => {
            const list = curated.filter((model) => model.provider === provider);
            if (list.length === 0) return null;
            return (
              <Fragment key={provider}>
                <GroupHead>
                  <ProviderGlyph provider={provider} className="size-icon-sm" />
                  <span className="text-foreground/80 font-medium">{VENDOR_LABELS[provider]}</span>
                  <span className="text-muted-foreground">
                    {list.length} {list.length === 1 ? "model" : "models"}
                  </span>
                </GroupHead>
                {rows(list)}
              </Fragment>
            );
          })}
          {others.length > 0 && (
            <>
              <GroupHead>
                <Flask aria-hidden className="text-muted-foreground size-icon-sm" />
                <span className="text-foreground/80 font-medium">Not in the registry</span>
                <span className="text-muted-foreground">
                  {others.length} {others.length === 1 ? "model" : "models"} · listed by the agents' CLIs; an
                  inherited one is rated from its family less {INHERITED_PENALTY} until it has outcomes
                </span>
              </GroupHead>
              {rows(others)}
            </>
          )}
          <div className="text-muted-foreground border-divider flex flex-wrap items-center justify-between gap-3 border-t px-6 py-3 text-2xs">
            <span className="flex flex-wrap items-center gap-5">
              <span className="flex items-center gap-1.5">
                <span className="bg-success/15 text-success rounded-xs px-1 font-mono">9.0</span>
                Best for that kind
              </span>
              <span className="flex items-center gap-1.5">
                <span className="text-foreground font-mono">8.2</span>
                Eligible
              </span>
              <span className="flex items-center gap-1.5">
                <span className="text-foreground/30 font-mono">7.4</span>
                Below the tier floor
              </span>
              <span className="flex items-center gap-1.5">
                <span className="text-warning">Legacy · −1</span>
                hidden from the picker
              </span>
            </span>
            <span>Unknown models start at 5 and run only as trials</span>
          </div>
        </div>
      </div>
    </section>
  );
}

function GroupHead({ children }: { children: ReactNode }) {
  return <div className="border-divider flex items-center gap-2 border-t px-6 pt-4 pb-2">{children}</div>;
}

function RatingRow({
  model,
  best,
  open,
  onToggle,
  learned,
  registry,
  category,
  projectName,
  candidate,
}: {
  model: MergedModel;
  best: ReadonlyMap<TaskCategory, number>;
  open: boolean;
  onToggle: () => void;
  learned: readonly Learned[];
  registry: RegistryInfo | null;
  category: TaskCategory;
  projectName: string | null;
  candidate: RouteCandidate | null;
}) {
  const rating = ratingWords(model);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <div data-slot="rating-row" className={cn(open && "bg-foreground/4")}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="border-divider hover:bg-foreground/3 grid w-full items-center gap-1 border-t px-6 py-2 text-start" style={COLUMNS}
      >
        <span className="flex min-w-0 items-center gap-1.5 text-sm">
          <Chevron aria-hidden className="text-muted-foreground size-icon-xs shrink-0" />
          <span className={cn("truncate", model.legacy && "text-foreground/65")}>{model.displayName}</span>
        </span>
        <span>
          <Badge
            variant={model.tier === "frontier" ? "default" : "secondary"}
            className={model.tier === "frontier" ? undefined : "font-normal"}
          >
            {TIER_LABELS[model.tier]}
          </Badge>
        </span>
        {ROUTED_KINDS.map((kind) => {
          const strength = strengthFor(model, kind);
          const dim = faded(model, kind);
          const top = strength !== null && !dim && best.get(kind) === strength;
          return (
            <span key={kind} className="flex justify-center font-mono">
              <span
                className={cn(
                  "rounded-xs px-1",
                  strength === null && "text-muted-foreground",
                  dim && "text-foreground/30",
                  top && "bg-success/15 text-success",
                )}
              >
                {strength === null ? "—" : strength.toFixed(1)}
              </span>
            </span>
          );
        })}
        <span className="text-muted-foreground text-end font-mono">
          {model.contextWindow === null ? "—" : formatTokens(model.contextWindow)}
        </span>
        <span className="text-muted-foreground text-end font-mono">{effortRange(model.efforts)}</span>
        <span className={cn("truncate text-end", rating.tone)}>{rating.text}</span>
      </button>
      {open && (
        <Profile
          model={model}
          learned={learned}
          registry={registry}
          category={category}
          projectName={projectName}
          candidate={candidate}
        />
      )}
    </div>
  );
}

/** A label and its value in the profile. */
function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex gap-3">
      <dt className="text-muted-foreground w-24 shrink-0">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}

function Caps({ children }: { children: ReactNode }) {
  return <h4 className="text-muted-foreground pb-1 text-2xs font-medium tracking-eyebrow uppercase">{children}</h4>;
}

/** "medium; high for code, review, merge": the effort it runs at most, then the exceptions. */
function defaultEfforts(model: MergedModel): string {
  const entries = Object.entries(model.defaultEffort) as [TaskCategory, string][];
  if (entries.length === 0) return "—";
  const counts = new Map<string, number>();
  for (const [, effort] of entries) counts.set(effort, (counts.get(effort) ?? 0) + 1);
  const usual = [...counts.entries()].toSorted((a, b) => b[1] - a[1])[0]![0];
  const others = new Map<string, TaskCategory[]>();
  for (const [kind, effort] of entries) {
    if (effort === usual) continue;
    others.set(effort, [...(others.get(effort) ?? []), kind]);
  }
  return [
    usual,
    ...[...others.entries()].map(
      ([effort, kinds]) => `${effort} for ${kinds.map((kind) => KIND_HEADS[kind].toLowerCase()).join(", ")}`,
    ),
  ].join("; ");
}

/** "Text, images". */
function modalityWords(list: readonly string[]): string {
  if (list.length === 0) return "—";
  const words = list.map((entry) => (entry === "image" ? "images" : entry)).join(", ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** A model's profile: its facts, its strength per kind, its area adjustments and what it learned. */
function Profile({
  model,
  learned,
  registry,
  category,
  projectName,
  candidate,
}: {
  model: MergedModel;
  learned: readonly Learned[];
  registry: RegistryInfo | null;
  category: TaskCategory;
  projectName: string | null;
  candidate: RouteCandidate | null;
}) {
  const plan = useApp(
    (s) => s.providers.view?.providers.find((entry) => entry.provider === model.provider)?.status?.plan ?? null,
  );
  const own = learned.filter((entry) => entry.provider === model.provider && entry.model === model.id);
  const here = own.find((entry) => entry.category === category) ?? null;
  const outcomes = own.reduce((sum, entry) => sum + entry.samples, 0);
  const quota = candidate ? quotaFactor(candidate) : null;
  const load = candidate ? loadFactor(candidate) : null;
  const ratingFrom =
    model.status === "curated" && registry ? `Registry r${registry.revision} · ${registry.updated}` : ratingWords(model).text;
  return (
    <div className="px-6 pb-4">
      <div className="bg-background border-divider rounded-settings grid grid-cols-4 gap-8 border p-5">
        <div className="flex min-w-0 flex-col gap-2">
          <Caps>Profile</Caps>
          <span className="flex items-center gap-2 text-base font-medium">
            <ProviderGlyph provider={model.provider} className="size-icon-md" />
            {model.displayName}
            <span className="border-divider text-muted-foreground truncate rounded-xs border px-1 font-mono text-2xs font-normal">
              {model.id}
            </span>
          </span>
          <dl className="flex flex-col gap-1.5">
            <Fact label="Provider">
              {VENDOR_LABELS[model.provider]}
              {plan && ` · ${plan.charAt(0).toUpperCase()}${plan.slice(1)}`}
            </Fact>
            <Fact label="Tier">{TIER_LABELS[model.tier]}</Fact>
            <Fact label="Context">
              {model.contextWindow === null ? "—" : `${formatTokens(model.contextWindow)} tokens`}
            </Fact>
            <Fact label="Efforts">{model.efforts.length > 0 ? model.efforts.join(" · ") : "its own"}</Fact>
            <Fact label="Default effort">{defaultEfforts(model)}</Fact>
            <Fact label="Takes in">{modalityWords(model.modalities.input)}</Fact>
            <Fact label="Puts out">{modalityWords(model.modalities.output)}</Fact>
            <Fact label="Rating from">{ratingFrom}</Fact>
          </dl>
        </div>
        <div className="flex min-w-0 flex-col gap-1.5">
          <Caps>Strength by kind</Caps>
          {ROUTED_KINDS.map((kind) => {
            const strength = strengthFor(model, kind);
            return (
              <div key={kind} className="flex items-center gap-3">
                <span className="text-muted-foreground w-20 shrink-0">{KIND_HEADS[kind]}</span>
                <span className="bg-foreground/10 h-1.5 flex-1 overflow-hidden rounded-full">
                  <span
                    className="bg-foreground/70 block h-full rounded-full"
                    style={{ width: `${(strength ?? 0) * 10}%` }}
                  />
                </span>
                <span className="w-7 text-end font-mono">{strength === null ? "—" : strength.toFixed(1)}</span>
              </div>
            );
          })}
        </div>
        <div className="flex min-w-0 flex-col gap-1.5">
          <Caps>Area adjustments</Caps>
          {AREAS.map((area: Area) => {
            const value = model.areaStrengths[area] ?? 0;
            return (
              <div key={area} className="flex items-center gap-3">
                <span className="text-muted-foreground w-16 shrink-0">{AREA_LABELS[area]}</span>
                <span className="relative h-1.5 flex-1">
                  <span className="bg-divider absolute inset-y-0 left-1/2 w-px" />
                  <span
                    className={cn(
                      "absolute inset-y-0 rounded-full",
                      value < 0 ? "bg-warning right-1/2" : "bg-success left-1/2",
                    )}
                    style={{ width: `${(Math.abs(value) / MAX_AREA) * 50}%` }}
                  />
                </span>
                <span
                  className={cn(
                    "w-9 text-end font-mono",
                    value < 0 ? "text-warning" : value > 0 ? "text-success" : "text-muted-foreground",
                  )}
                >
                  {value > 0 ? "+" : value < 0 ? "−" : ""}
                  {Math.abs(value).toFixed(1)}
                </span>
              </div>
            );
          })}
          <p className="text-muted-foreground pt-1 text-2xs">
            Up to ±{MAX_AREA} per area. A task touching several areas gets the mean.
          </p>
        </div>
        <div className="flex min-w-0 flex-col gap-2">
          <Caps>{`In ${projectName ?? "all projects"}`}</Caps>
          <span className="flex items-baseline gap-2">
            <span
              className={cn(
                "font-mono text-2xl",
                here && here.adjustment > 0 ? "text-success" : here && here.adjustment < 0 ? "text-warning" : "text-foreground/65",
              )}
            >
              {formatPart(here?.adjustment ?? 0)}
            </span>
            <span className="text-muted-foreground">
              learned for {KIND_HEADS[category].toLowerCase()}, of ±{MAX_LEARNED} max
            </span>
          </span>
          <dl className="flex flex-col gap-1.5">
            <Fact label="Outcomes">
              {outcomes === 0
                ? "none yet"
                : `${outcomes} · ${own
                    .filter((entry) => entry.samples > 0)
                    .map((entry) => `${entry.samples} ${KIND_HEADS[entry.category].toLowerCase()}`)
                    .join(", ")}`}
            </Fact>
            {here && (
              <Fact label="Quality">
                {[
                  `${Math.round(here.successRate * 100)}% succeeded`,
                  here.reviewPassRate !== null && `${Math.round(here.reviewPassRate * 100)}% passed review first time`,
                  here.avgRework !== null && `${here.avgRework.toFixed(1)} rework rounds`,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </Fact>
            )}
          </dl>
          <div className="bg-divider my-1 h-px" />
          {candidate?.heat && (
            <span className="flex items-center gap-1.5">
              <span
                aria-hidden
                className={cn(
                  "size-1.5 rounded-full",
                  candidate.heat === "cool" ? "bg-success" : candidate.heat === "warm" ? "bg-foreground/70" : candidate.heat === "hot" ? "bg-warning" : "bg-destructive",
                )}
              />
              {HEAT_LABELS[candidate.heat]}
            </span>
          )}
          <dl className="flex flex-col gap-1.5">
            <Fact label="Quota penalty">{quota ? `${formatPart(quota.delta)} · ${quota.label}` : "none"}</Fact>
            <Fact label="Load">{load ? `${formatPart(load.delta)} · ${load.label}` : "none"}</Fact>
          </dl>
        </div>
      </div>
    </div>
  );
}

/** "Refresh rankings": starts a refresh, or shows that one is running. */
function RefreshButton({ rankings }: { rankings: RankingsRefreshView }) {
  const { refresh, starting, start } = rankings;
  const running = refresh !== null && refreshRunning(refresh.state);
  return (
    <SettingsButton disabled={running || starting.busy || refresh === null} onClick={() => starting.run(start)}>
      <Reload className={running || starting.busy ? "animate-spin motion-reduce:animate-none" : undefined} />
      {running ? "Refreshing…" : RATINGS_ROWS.refresh.label}
    </SettingsButton>
  );
}

/** The display name of the model a refresh researched with, when the models list has it. */
function researcherName(refresh: RankingsRefresh, models: readonly MergedModel[] | null): string | null {
  if (refresh.model === null) return null;
  const model = models?.find(
    (entry) => entry.provider === refresh.provider && (entry.id === refresh.model || entry.resolved === refresh.model),
  );
  return model?.displayName ?? refresh.model;
}

/**
 * Under the header: what a refresh is doing or how the last one ended, the ratings in use with
 * the way back to the published ones, what the last refresh changed, and what went wrong.
 */
function RefreshLines({
  rankings,
  models,
  registry,
}: {
  rankings: RankingsRefreshView;
  models: readonly MergedModel[] | null;
  registry: RegistryInfo | null;
}) {
  const { refresh, error, starting, reset } = rankings;
  const [resetting, setResetting] = useState(false);
  const running = refresh !== null && refreshRunning(refresh.state);
  const status = refresh && refreshStatus(refresh, researcherName(refresh, models));
  const failure = starting.error ?? error;
  const changed = refresh?.state === "done" ? changedModels(refresh.changes) : [];
  return (
    <div className="flex flex-col gap-1 px-6 pb-3 text-xs empty:hidden">
      {failure && (
        <p role="alert" className="text-destructive">
          {failure}
        </p>
      )}
      {/* Always there, so screen readers hear each change of it. */}
      <p role="status" aria-live="polite" className={cn("empty:hidden", status && STATUS_TONES[status.tone])}>
        {status?.text}
      </p>
      {registry?.error && <p className="text-warning">The last registry check didn't update it: {registry.error}</p>}
      {refresh?.overlayApplied && (
        <p className="text-muted-foreground flex flex-wrap items-center gap-2">
          {overlayStatus(refresh)}
          <Button type="button" size="xs" variant="ghost" className="text-link" onClick={() => setResetting(true)}>
            Use published ratings
          </Button>
        </p>
      )}
      {changed.length > 0 && (
        <Collapsible>
          <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex items-center gap-1">
            <ChevronRight
              aria-hidden
              className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
            />
            What changed
          </CollapsibleTrigger>
          <CollapsibleContent>
            <ul className="flex flex-col gap-2 py-2 ps-5">
              {changed.map((change) => {
                const name =
                  models?.find(
                    (entry) =>
                      entry.provider === change.provider && (entry.id === change.model || entry.resolved === change.model),
                  )?.displayName ?? change.model;
                return (
                  <li key={`${change.provider}/${change.model}`} className="flex min-w-0 flex-col gap-0.5">
                    <span>
                      <span className="font-medium">{name}</span>
                      <span className="text-muted-foreground">: {change.fields.map(fieldWords).join(", ")}</span>
                    </span>
                    {change.sources.map((source) => (
                      <SourceLink key={source} url={source} />
                    ))}
                  </li>
                );
              })}
            </ul>
          </CollapsibleContent>
        </Collapsible>
      )}
      {refresh && refresh.errors.length > 0 && !running && (
        <p className={refresh.state === "failed" ? "text-destructive" : "text-warning"}>
          {refresh.state === "failed" ? "What went wrong: " : "Some findings were left out: "}
          {refresh.errors.join("; ")}
        </p>
      )}
      <ResetRatingsDialog open={resetting} running={running} onOpenChange={setResetting} onConfirm={reset} />
    </div>
  );
}

function ResetRatingsDialog({
  open,
  running,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  running: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await onConfirm();
      onOpenChange(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Use the published ratings?</DialogTitle>
          <DialogDescription>
            The researched ratings are discarded and every model goes back to its published rating.
            {running && " The refresh that is running stops."} Your own choices stay as they are.
          </DialogDescription>
        </DialogHeader>
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        <DialogFooter>
          <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" size="sm" disabled={busy} onClick={() => void confirm()}>
            Use published ratings
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
