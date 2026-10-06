import { ChevronRight, Clock, DotsHorizontal, Plus, Trash } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { ExplanationView } from "@/app/conversation/cards/RouteDetails";
import { useAction } from "@/app/conversation/useAction";
import { ManualList } from "@/app/routing/ManualList";
import { type Choice, Segmented } from "@/app/settings/parts";
import { HeatBadge } from "@/app/usage/UsagePage";
import { effortLabel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useNow } from "@/hooks/use-now";
import type {
  Area,
  Factor,
  Ranking,
  RouteCandidate,
  RoutePreview,
  TaskCategory,
} from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import {
  AREA_LABELS,
  AREAS,
  CATEGORY_LABELS,
  choiceName,
  formatDelta,
  formatResetAt,
  formatScore,
  KIND_HINTS,
  newRuleId,
  PLAIN_KIND_NAMES,
  ruleSentence,
} from "@/lib/routing";
import { cn } from "@/lib/utils";
import { addOverride, changeRanking, newRankingId, useRoutePreview } from "@/state/routing";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

type Mode = "inherit" | "automatic" | "manual";

/** Rows an Automatic list shows before "Show all". */
const SHOWN_CANDIDATES = 4;

/** The places a new Manual list starts with: what Automatic would try first. */
const SEEDED_PLACES = 3;

/** The ranking a scope holds for a kind of work's own row (not an area override). */
function rowRanking(
  rankings: readonly Ranking[],
  category: TaskCategory,
  scope: string | null,
): Ranking | null {
  return (
    rankings.find(
      (ranking) =>
        ranking.category === category && ranking.projectId === scope && ranking.areas.length === 0,
    ) ?? null
  );
}

/** A Manual list's first places: the models Automatic would try first, as they run now. */
function seed(route: RoutePreview | null): Ranking["entries"] {
  return (route?.candidates ?? [])
    .filter((candidate) => candidate.blocked === null)
    .slice(0, SEEDED_PLACES)
    .map((candidate) => ({
      target: { type: "model", provider: candidate.provider, id: candidate.model },
      effort: null,
    }));
}

/** How a row's mode reads in its summary. */
const MODE_LABELS: Record<Exclude<Mode, "inherit">, string> = {
  automatic: "Automatic",
  manual: "Manual",
};

/**
 * One kind of work, as a row of the Routing page's card: its name, what its next task would run
 * on and whether routing ranks the models (Automatic) or tries the user's list (Manual). Opening
 * it shows the order and why, the choice between the two, and its area overrides, everywhere or
 * in the chosen project.
 */
export function CategoryCard({
  category,
  route,
  scope,
  groups,
}: {
  category: TaskCategory;
  /** The live preview for this kind of work; `null` while it is read. */
  route: RoutePreview | null;
  /** The project the page is about; `null`: everywhere. */
  scope: string | null;
  groups: readonly ModelGroup[];
}) {
  const rankings = useApp((s) => s.settings.routingRankings);
  const own = rowRanking(rankings, category, scope);
  const everywhere = scope === null ? null : rowRanking(rankings, category, null);
  const mode: Mode = own ? (own.manual ? "manual" : "automatic") : scope ? "inherit" : "automatic";
  const action = useAction();
  const name = PLAIN_KIND_NAMES[category];

  const setMode = (next: Mode) =>
    action.run(async () => {
      if (next === "inherit") {
        if (own) await changeRanking(own.id, () => null);
        return;
      }
      const manual = next === "manual";
      const create = (): Ranking => ({
        id: newRankingId(),
        category,
        areas: [],
        projectId: scope,
        manual,
        // A project's list starts as the one everywhere, else as Automatic's order.
        entries: everywhere?.entries.length ? everywhere.entries : seed(route),
        only: everywhere?.only ?? false,
        updatedAtMs: Date.now(),
      });
      await changeRanking(
        own?.id ?? "",
        (current) => ({
          ...current,
          manual,
          entries: manual && current.entries.length === 0 ? seed(route) : current.entries,
        }),
        create,
      );
    });

  const options: Choice<Mode>[] = [
    ...(scope ? [{ value: "inherit" as const, label: "As everywhere" }] : []),
    { value: "automatic", label: "Automatic" },
    { value: "manual", label: "Manual" },
  ];
  // The list in force here decides what the live preview's places are about.
  const livePlaces = own && route?.rankingId === own.id ? route.places : null;

  const overrides = rankings.filter(
    (ranking) =>
      ranking.category === category && ranking.projectId === scope && ranking.areas.length > 0,
  ).length;
  const modeLabel =
    mode === "inherit"
      ? `As everywhere · ${everywhere?.manual ? "Manual" : "Automatic"}`
      : MODE_LABELS[mode];

  return (
    <Collapsible data-slot="routing-kind">
      <CollapsibleTrigger className="hover:bg-foreground/4 group flex w-full items-center gap-3 px-4 py-3 text-start">
        <ChevronRight
          aria-hidden
          className="text-muted-foreground size-icon-xs shrink-0 transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
        />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-label truncate font-medium">{name}</span>
          <span className="text-foreground/65 truncate text-xs" title={KIND_HINTS[category]}>
            {KIND_HINTS[category]}
          </span>
        </span>
        <span className="flex max-w-1/2 min-w-0 shrink-0 flex-col items-end gap-0.5 text-end">
          <NextSummary route={route} groups={groups} />
          <span className="text-muted-foreground truncate text-xs">
            {modeLabel}
            {overrides > 0 && ` · ${overrides} area ${overrides === 1 ? "list" : "lists"}`}
          </span>
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent className="bg-foreground/2 border-divider flex flex-col border-t">
        <div className="flex items-center gap-3 px-4 pt-3">
          <span className="text-foreground/65 min-w-0 flex-1 text-xs">
            {mode === "manual"
              ? "Your list, tried top-down: the first model with quota to spare takes the task."
              : mode === "inherit"
                ? `Follows the setting everywhere (${everywhere?.manual ? "your list" : "Automatic"}).`
                : "Routing ranks the models by strength, what worked here and quota."}
          </span>
          <Segmented
            label={`How ${name} work is routed`}
            value={mode}
            options={options}
            onChange={setMode}
          />
        </div>
        {mode === "manual" && own ? (
          <ManualList
            ranking={own}
            category={category}
            places={livePlaces}
            candidates={route?.candidates ?? []}
            groups={groups}
          />
        ) : (
          <AutomaticList
            category={category}
            route={route}
            scope={scope}
            groups={groups}
            onRank={() => setMode("manual")}
          />
        )}
        <Outcome route={route} groups={groups} />
        {action.error && (
          <p role="alert" className="text-destructive px-4 pb-3 text-xs">
            {action.error}
          </p>
        )}
        <AreaOverrides category={category} scope={scope} route={route} groups={groups} />
      </CollapsibleContent>
    </Collapsible>
  );
}

/** What the next task of a kind would run on, or that it would wait, on the row's end. */
function NextSummary({ route, groups }: { route: RoutePreview | null; groups: readonly ModelGroup[] }) {
  const now = useNow(30_000);
  if (!route) return <span className="text-muted-foreground text-xs">Reading…</span>;
  const { outcome } = route;
  if (outcome.type === "wait") {
    return (
      <span className="text-warning text-label flex items-center gap-1 truncate">
        <Clock aria-hidden className="size-icon-xs shrink-0" />
        Waits
        {outcome.resetsAtMs !== null && ` · ${formatCountdown(outcome.resetsAtMs, now)}`}
      </span>
    );
  }
  return (
    <span className="text-label truncate">
      {choiceName(groups, outcome.choice)}
      {outcome.choice.effort && (
        <span className="text-muted-foreground"> · {effortLabel(outcome.choice.effort)}</span>
      )}
    </span>
  );
}

/** A factor's short name for the list ("learned +0.8", "quota −1.2"). */
function factorNote(factor: Factor): string | null {
  const label = factor.label;
  const delta = formatDelta(factor.delta);
  if (label.startsWith("learned here")) return `learned ${delta}`;
  if (label.includes("projected at")) return `quota ${delta}`;
  if (label.includes("already runs")) return `busy ${delta}`;
  if (label.startsWith("trial")) return `trial ${delta}`;
  if (label.startsWith("strength for")) return `area ${delta}`;
  if (label.startsWith("an older model")) return `older ${delta}`;
  if (label.startsWith("a newer model")) return `not yet rated ${delta}`;
  return null;
}

/** What a candidate's score is made of, in a few words. */
function scoreNote(candidate: RouteCandidate): string {
  const [strength, ...rest] = candidate.factors;
  const parts = [
    strength ? `strength ${formatScore(strength.delta)}` : null,
    ...rest.map(factorNote),
  ].filter((part) => part !== null);
  return parts.join(" · ");
}

/**
 * Automatic: the models in the order routing would try them for the next task, with the
 * effort, the provider's heat and what their score is made of; those that can't take it last,
 * with why. Each can be kept from this kind of work without leaving Automatic.
 */
function AutomaticList({
  category,
  route,
  scope,
  groups,
  onRank,
}: {
  category: TaskCategory;
  route: RoutePreview | null;
  scope: string | null;
  groups: readonly ModelGroup[];
  onRank: () => void;
}) {
  const [all, setAll] = useState(false);
  const now = useNow(60_000);
  if (!route) {
    return <p className="text-muted-foreground px-4 py-3 text-xs">Reading what routing would pick…</p>;
  }
  // A manual list decides here (an area override for the previewed areas, or the one everywhere).
  const listed = route.places.length > 0;
  const candidates = route.candidates;
  const shown = all ? candidates : candidates.slice(0, SHOWN_CANDIDATES);
  return (
    <div data-slot="automatic-ranking" className="flex flex-col px-4 py-2">
      {listed && (
        <p className="text-muted-foreground py-1 text-xs">
          A Manual list decides for the areas previewed; this is the order it gives.
        </p>
      )}
      <ol className="flex flex-col">
        {shown.map((candidate, index) => {
          // Those that can run come first, in the order routing tries them.
          const runs = candidate.blocked === null;
          const name = choiceName(groups, { provider: candidate.provider, model: candidate.model, effort: null });
          return (
            <li
              key={`${candidate.provider}/${candidate.model}`}
              data-slot="route-candidate"
              data-chosen={candidate.chosen || undefined}
              className="flex items-center gap-2 py-1.5"
            >
              <span className="text-muted-foreground w-5 shrink-0 text-xs tabular-nums">
                {runs ? `${index + 1}` : "–"}
              </span>
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="flex min-w-0 items-center gap-1.5">
                  <span className={cn("text-label truncate", runs ? "font-medium" : "text-muted-foreground")}>
                    {name}
                  </span>
                  {candidate.effort && (
                    <span className="text-muted-foreground shrink-0 text-xs">{effortLabel(candidate.effort)}</span>
                  )}
                  {candidate.chosen && <Badge variant="secondary">Next</Badge>}
                  {candidate.trial && <Badge variant="warning">Trial</Badge>}
                  {candidate.new && !candidate.trial && (
                    <Badge variant="outline" title="A new model: it runs only as a trial until rated here">
                      New
                    </Badge>
                  )}
                </span>
                <span className="text-muted-foreground truncate text-xs" title={candidate.blocked ?? undefined}>
                  {candidate.blocked
                    ? candidate.resetsAtMs !== null
                      ? `${candidate.blocked} · ${formatCountdown(candidate.resetsAtMs, now)} left`
                      : candidate.blocked
                    : scoreNote(candidate)}
                </span>
              </div>
              {candidate.heat && candidate.heat !== "cool" && <HeatBadge heat={candidate.heat} />}
              {candidate.score !== null && (
                <span className="text-muted-foreground w-8 shrink-0 text-end text-xs tabular-nums">
                  {formatScore(candidate.score)}
                </span>
              )}
              <CandidateMenu
                candidate={candidate}
                name={name}
                category={category}
                scope={scope}
                groups={groups}
                onRank={onRank}
              />
            </li>
          );
        })}
      </ol>
      {candidates.length > SHOWN_CANDIDATES && (
        <Button type="button" size="xs" variant="ghost" className="w-fit" onClick={() => setAll(!all)}>
          {all ? "Show fewer" : `Show all ${candidates.length}`}
        </Button>
      )}
      {route.trialSlot && (
        <p className="text-muted-foreground py-1 text-xs">
          The next {CATEGORY_LABELS[category]} task may try out a new model.
        </p>
      )}
    </div>
  );
}

/** "Don't use for…" (a rule, the row stays Automatic) and "Rank manually". */
function CandidateMenu({
  candidate,
  name,
  category,
  scope,
  groups,
  onRank,
}: {
  candidate: RouteCandidate;
  name: string;
  category: TaskCategory;
  scope: string | null;
  groups: readonly ModelGroup[];
  onRank: () => void;
}) {
  const projects = useApp((s) => s.projects);
  const never = () => {
    const rule = {
      id: newRuleId(),
      effect: "never" as const,
      target: { type: "model" as const, provider: candidate.provider, id: candidate.model },
      categories: [category],
      areas: [],
      projectId: scope,
      createdAtMs: Date.now(),
    };
    const sentence = ruleSentence(rule, groups, projects);
    addOverride(rule)
      .then(() => toast(`Added: ${sentence}.`))
      .catch((cause: unknown) =>
        toast(`Couldn't add the rule: ${cause instanceof Error ? cause.message : String(cause)}`, {
          tone: "error",
        }),
      );
  };
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button type="button" size="icon-xs" variant="ghost" aria-label={`${name}: actions`} className="text-muted-foreground">
          <DotsHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>{name}</DropdownMenuLabel>
        <DropdownMenuItem onSelect={never}>Don't use for {CATEGORY_LABELS[category]}</DropdownMenuItem>
        <DropdownMenuItem onSelect={onRank}>Rank models by hand</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** What the next task of this kind would run on, or what it would wait for, with the details. */
function Outcome({ route, groups }: { route: RoutePreview | null; groups: readonly ModelGroup[] }) {
  const now = useNow(30_000);
  if (!route) return null;
  const { outcome } = route;
  if (outcome.type === "wait") {
    return (
      <div className="flex flex-col gap-0.5 px-4 pb-3 text-xs">
        <span className="text-warning flex items-center gap-1">
          <Clock aria-hidden className="size-icon-xs shrink-0" />
          The next task waits
          {outcome.resetsAtMs !== null &&
            ` until ${formatResetAt(outcome.resetsAtMs, now)} (${formatCountdown(outcome.resetsAtMs, now)})`}
        </span>
        <span className="text-muted-foreground">{outcome.reason}</span>
      </div>
    );
  }
  return (
    <Collapsible className="px-4 pb-3 text-xs">
      <div className="flex items-baseline gap-1">
        <span className="text-muted-foreground min-w-0 flex-1">
          <span className="text-foreground">Why {choiceName(groups, outcome.choice)}:</span>{" "}
          {outcome.reason}
        </span>
        {outcome.explanation && (
          <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex shrink-0 items-center gap-1">
            <ChevronRight
              aria-hidden
              className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
            />
            Details
          </CollapsibleTrigger>
        )}
      </div>
      {outcome.explanation && (
        <CollapsibleContent className="mt-2">
          <ExplanationView explanation={outcome.explanation} groups={groups} />
        </CollapsibleContent>
      )}
    </Collapsible>
  );
}

/**
 * Area overrides: for tasks touching some areas, a list of their own (Frontend + Implement:
 * Opus first). The most specific applies; they are tried like the row's Manual list.
 */
function AreaOverrides({
  category,
  scope,
  route,
  groups,
}: {
  category: TaskCategory;
  scope: string | null;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
}) {
  const rankings = useApp((s) => s.settings.routingRankings);
  const overrides = rankings.filter(
    (ranking) =>
      ranking.category === category && ranking.projectId === scope && ranking.areas.length > 0,
  );
  const action = useAction();
  const row = rowRanking(rankings, category, scope);
  const add = (area: Area) =>
    action.run(() =>
      changeRanking("", (created) => created, () => ({
        id: newRankingId(),
        category,
        areas: [area],
        projectId: scope,
        manual: true,
        entries: row?.entries.length ? row.entries : seed(route),
        only: false,
        updatedAtMs: Date.now(),
      })),
    );
  return (
    <Collapsible>
      <div className="flex items-center gap-2 px-4 py-1.5">
        <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex min-w-0 flex-1 items-center gap-1 text-xs">
          <ChevronRight
            aria-hidden
            className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
          />
          Area overrides{overrides.length > 0 && ` (${overrides.length})`}
        </CollapsibleTrigger>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button type="button" size="xs" variant="ghost">
              <Plus />
              Add
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuLabel>A list for tasks touching</DropdownMenuLabel>
            {AREAS.map((area) => (
              <DropdownMenuItem key={area} onSelect={() => add(area)}>
                {AREA_LABELS[area].charAt(0).toUpperCase()}
                {AREA_LABELS[area].slice(1)}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive px-4 pb-2 text-xs">
          {action.error}
        </p>
      )}
      <CollapsibleContent className="flex flex-col">
        {overrides.length === 0 ? (
          <p className="text-muted-foreground px-4 pb-3 text-xs">
            None: tasks touching any area follow the row above.
          </p>
        ) : (
          overrides.map((override) => (
            <AreaOverride key={override.id} ranking={override} category={category} scope={scope} groups={groups} />
          ))
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Whether an area override applies; switched off, its list is kept for switching back. */
const OVERRIDE_STATES: Choice<"on" | "off">[] = [
  { value: "on", label: "On" },
  { value: "off", label: "Off" },
];

/**
 * One area override: its areas, whether it applies, its list (with its own live preview), and
 * removing it.
 */
function AreaOverride({
  ranking,
  category,
  scope,
  groups,
}: {
  ranking: Ranking;
  category: TaskCategory;
  scope: string | null;
  groups: readonly ModelGroup[];
}) {
  const { routes } = useRoutePreview(scope, ranking.areas);
  const route = routes?.find((found) => found.category === category) ?? null;
  const action = useAction();
  const places = route?.rankingId === ranking.id ? route.places : null;
  return (
    <div data-slot="area-override" className="border-divider flex flex-col border-t">
      <div className="flex items-center gap-2 px-4 pt-3">
        <span className="text-muted-foreground text-xs">Tasks touching</span>
        <ToggleGroup
          type="multiple"
          size="sm"
          variant="outline"
          spacing="tight"
          aria-label="Areas this list is for"
          className="flex-wrap"
          value={ranking.areas}
          onValueChange={(value) =>
            value.length > 0 &&
            action.run(() => changeRanking(ranking.id, (current) => ({ ...current, areas: value as Area[] })))
          }
        >
          {AREAS.map((area) => (
            <ToggleGroupItem key={area} value={area} className="text-xs">
              {AREA_LABELS[area]}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <div className="ms-auto">
          <Segmented
            label="Whether this area override applies"
            value={ranking.manual ? "on" : "off"}
            options={OVERRIDE_STATES}
            disabled={action.busy}
            onChange={(next) =>
              action.run(() =>
                changeRanking(ranking.id, (current) => ({ ...current, manual: next === "on" })),
              )
            }
          />
        </div>
        <Button
          type="button"
          size="icon-xs"
          variant="ghost"
          aria-label="Remove this area override"
          className="text-muted-foreground"
          disabled={action.busy}
          onClick={() => action.run(() => changeRanking(ranking.id, () => null))}
        >
          <Trash />
        </Button>
      </div>
      {ranking.manual ? (
        <>
          <ManualList
            ranking={ranking}
            category={category}
            places={places}
            candidates={route?.candidates ?? []}
            groups={groups}
          />
          <Outcome route={route} groups={groups} />
        </>
      ) : (
        <p className="text-muted-foreground px-4 py-3 text-xs">
          Off: tasks touching {ranking.areas.length === 1 ? "this area" : "these areas"} follow the
          row above. The list is kept for when you switch it back on.
        </p>
      )}
      {action.error && (
        <p role="alert" className="text-destructive px-4 pb-2 text-xs">
          {action.error}
        </p>
      )}
    </div>
  );
}
