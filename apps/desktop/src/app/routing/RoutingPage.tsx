import { ChevronDown, ChevronRight, Clock } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect, useState } from "react";

import { AREA_OVERRIDES_ROW, AreaOverrides } from "@/app/routing/AreaOverrides";
import { ModelRatings, RATINGS_ROWS } from "@/app/routing/ModelRatings";
import { useScopeOptions } from "@/app/routing/pickers";
import { RankingNow, type RankingQuestion } from "@/app/routing/RankingNow";
import { RoutingRules } from "@/app/routing/RoutingRules";
import { TryOrder } from "@/app/routing/TryOrder";
import {
  areaOverrides,
  authorPicks,
  decides,
  KIND_ICONS,
  orderSummary,
  ROUTED_KINDS,
  rowRanking,
  shortModelName,
} from "@/app/routing/words";
import {
  SettingsAdvanced,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
} from "@/app/settings/parts";
import { revealSetting } from "@/app/settings/reveal";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { useNow } from "@/hooks/use-now";
import type { Area, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { KIND_HINTS, PLAIN_KIND_NAMES } from "@/lib/routing";
import { useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { openSettings } from "@/state/actions";
import { useRankingsRefresh } from "@/state/rankings";
import { useRoutePreview } from "@/state/routing";
import { useApp } from "@/state/store";
import { setUsageProject, useUsage, useUsageRefresh } from "@/state/usage";

/** The Routing page's rows, for Settings search; the page renders this copy. */
export const ROUTING_PAGE_ROWS = {
  kinds: {
    label: "Who does what",
    description: "Which model takes each kind of work Brigadier hands out: Automatic, or your order",
  },
  steps: {
    label: "How a model is picked",
    description: "What can't run is skipped, then your rules, the tier floor, your order, then the best score",
  },
  scope: {
    label: "Settings for",
    description: "A project can have its own orders and rules; the others follow All projects",
  },
  order: {
    label: "Order",
    description: "The models a kind of work tries, in order, their effort, and what happens when none can run",
    target: "Who does what",
  },
  ranking: {
    label: "Ranking right now",
    description: "Every model scored for the next task: strength, area, learned, quota and load",
  },
  rules: {
    label: "Rules",
    description: "Never use a model for some work, don't give it work at all, prefer it, or only use it",
  },
  overrides: AREA_OVERRIDES_ROW,
  ratings: RATINGS_ROWS.ratings,
  refresh: { ...RATINGS_ROWS.refresh, target: RATINGS_ROWS.ratings.label },
} as const;

/** The steps routing narrows the models by, in order, in plain words. */
const STEPS: readonly [string, string][] = [
  ["Skip what can't run", "Signed out, at a limit, just failed, or can't handle what the work needs, like images"],
  ["Follow your rules", "A model you said never to use for this is left out; when you said to use only some, only those are left"],
  [
    "Keep important work on strong models",
    "Writing code, reviewing changes and resolving conflicts need a Frontier model; your order may go below that",
  ],
  ["Use your order", "When the kind of work has one: the first model in it that can run"],
  [
    "Otherwise, the best score",
    "A model you prefer first; else strength for the work plus what Brigadier learned, less a model near its limit or busy",
  ],
];

/** The Advanced rows Settings search may bring into view, which open it. */
const ADVANCED_LABELS = [
  ROUTING_PAGE_ROWS.steps.label,
  ROUTING_PAGE_ROWS.scope.label,
  ROUTING_PAGE_ROWS.ranking.label,
  ROUTING_PAGE_ROWS.overrides.label,
  ROUTING_PAGE_ROWS.ratings.label,
];

/**
 * Settings › Routing: who does what, a row per kind of work saying what it runs on, which opens
 * in place to Automatic or your order; the rules; then, under Advanced, how a model is picked,
 * settings for a project, the ranking right now, area overrides and every model's ratings.
 */
export function RoutingPage() {
  useUsageRefresh();
  const groups = useModelGroups();
  const projects = useApp((s) => s.projects);
  const rankings = useApp((s) => s.settings.routingRankings);
  const view = useUsage((s) => s.view);
  const ratings = useRankingsRefresh();
  const now = useNow(30_000);
  const scopes = useScopeOptions();
  // The kind opened in Who does what; `kind` is the one the ranking and ratings are about.
  const [open, setOpen] = useState<TaskCategory | null>(null);
  const [kind, setKind] = useState<TaskCategory>("scout");
  const [scope, setScope] = useState<string | null>(null);
  const [question, setQuestion] = useState<RankingQuestion>({ projectId: null, area: null, author: "claude" });
  const { routes, error } = useRoutePreview(scope, NO_AREAS);
  const ranked = useRoutePreview(question.projectId, question.area ? [question.area] : NO_AREAS, question.author);

  // The learned numbers follow the project the ranking is scored for.
  useEffect(() => {
    if (useUsage.getState().projectId !== question.projectId) setUsageProject(question.projectId);
  }, [question.projectId]);

  const changeScope = (next: string | null) => {
    setScope(next);
    setQuestion((current) => ({ ...current, projectId: next }));
  };
  const toggle = (category: TaskCategory) => {
    setOpen(open === category ? null : category);
    setKind(category);
  };
  // The ranking behind a row's pick: its kind, in the scope the rows show, touching no area.
  const why = (category: TaskCategory) => {
    setKind(category);
    setQuestion((current) => ({ ...current, projectId: scope, area: null }));
    revealSetting(ROUTING_PAGE_ROWS.ranking.label);
  };
  const scopeName = scope ? (projects[scope]?.name ?? "this project") : null;

  return (
    <SettingsPage
      title="Routing"
      description="Which model takes each kind of work. Brigadier picks automatically; change only what you want to."
    >
      {error && (
        <p role="alert" className="text-destructive text-xs">
          {error}
        </p>
      )}
      <SettingsSection
        title={ROUTING_PAGE_ROWS.kinds.label}
        description={
          scopeName
            ? `In ${scopeName}; other projects follow All projects. Changes apply to new work.`
            : "Automatic picks the strongest model that can run now. Changes apply to new work."
        }
      >
        <SettingsCard>
          {ROUTED_KINDS.map((category) => (
            <KindRow
              key={category}
              category={category}
              open={category === open}
              route={routeOf(category, routes)}
              scope={scope}
              rankings={rankings}
              groups={groups}
              now={now}
              onToggle={() => toggle(category)}
            >
              {category === "gitText" && <GitWriterNote groups={groups} />}
              <TryOrder
                category={category}
                scope={scope}
                own={rowRanking(rankings, category, scope)}
                everywhere={scope === null ? null : rowRanking(rankings, category, null)}
                route={routeOf(category, routes)}
                groups={groups}
                onWhy={() => why(category)}
              />
            </KindRow>
          ))}
        </SettingsCard>
      </SettingsSection>
      <RoutingRules category={open} scope={scope} groups={groups} />
      <SettingsAdvanced
        description="How a model is picked, settings for a project, the ranking right now, area overrides and every model's ratings"
        labels={ADVANCED_LABELS}
      >
        <SettingsSection title={ROUTING_PAGE_ROWS.steps.label} description="Each step narrows what the next one sees">
          <SettingsCard>
            {STEPS.map(([step, hint], index) => (
              <SettingsRow key={step} label={`${index + 1}. ${step}`} description={hint} />
            ))}
          </SettingsCard>
        </SettingsSection>
        <SettingsCard>
          <SettingsRow label={ROUTING_PAGE_ROWS.scope.label} description={ROUTING_PAGE_ROWS.scope.description}>
            <SettingsSelect
              label="Where these settings apply"
              value={scope ?? ""}
              options={scopes}
              onChange={(value) => changeScope(value === "" ? null : value)}
            />
          </SettingsRow>
        </SettingsCard>
        <div className="flex flex-col gap-2">
          {ranked.error && (
            <p role="alert" className="text-destructive text-xs">
              {ranked.error}
            </p>
          )}
          <RankingNow
            category={kind}
            route={routeOf(kind, ranked.routes)}
            question={question}
            onQuestion={setQuestion}
            onCategory={setKind}
            groups={groups}
          />
        </div>
        <AreaOverrides scope={scope} routes={routes} groups={groups} />
        <ModelRatings
          models={view?.models ?? null}
          learned={view && view.projectId === question.projectId ? view.learned : []}
          registry={view?.registry ?? null}
          rankings={ratings}
          category={kind}
          projectName={question.projectId ? (projects[question.projectId]?.name ?? null) : null}
          candidates={routeOf(kind, ranked.routes)?.candidates ?? []}
          now={now}
        />
      </SettingsAdvanced>
    </SettingsPage>
  );
}

const NO_AREAS: Area[] = [];

/** A kind of work's preview, from a preview of every kind. */
function routeOf(category: TaskCategory, list: RoutePreview[] | null): RoutePreview | null {
  return list?.find((found) => found.category === category) ?? null;
}

/** One kind of work: its name and what it runs on now; opened, its order in place. */
function KindRow({
  category,
  open,
  route,
  scope,
  rankings,
  groups,
  now,
  onToggle,
  children,
}: {
  category: TaskCategory;
  open: boolean;
  route: RoutePreview | null;
  scope: string | null;
  rankings: readonly Ranking[];
  groups: readonly ModelGroup[];
  now: number;
  onToggle: () => void;
  children: ReactNode;
}) {
  const gitWriter = useApp((s) => s.settings.gitWriter);
  const Icon = KIND_ICONS[category];
  const Chevron = open ? ChevronDown : ChevronRight;
  const own = rowRanking(rankings, category, scope);
  // A project without a row of its own follows All projects'; one set to Automatic doesn't.
  const followsAll = scope !== null && own === null;
  const list = decides(own) ? own : followsAll ? rowRanking(rankings, category, null) : null;
  const overrides = areaOverrides(rankings, category, scope).length;
  let line: ReactNode;
  if (category === "gitText" && gitWriter) {
    line = `Set in Git · ${shortModelName(groups, gitWriter.provider, gitWriter.model ?? "")}`;
  } else if (route?.outcome.type === "wait") {
    line = (
      <span className="text-warning inline-flex items-center gap-1">
        <Clock aria-hidden className="size-icon-xs" />
        Waits{route.outcome.resetsAtMs !== null && ` · ${formatCountdown(route.outcome.resetsAtMs, now)}`}
      </span>
    );
  } else if (decides(list)) {
    line = `${list === own ? "Your order" : "All projects' order"} · ${orderSummary(list, groups)}`;
  } else if (category === "chat") {
    line = "The chat's own model, then Automatic";
  } else if (authorPicks(route, groups, { effort: false })) {
    line = `Automatic · adversarial: ${authorPicks(route, groups, { effort: false })}`;
  } else if (route?.outcome.type === "chosen") {
    line = `Automatic · ${shortModelName(groups, route.outcome.choice.provider, route.outcome.choice.model ?? "")} now`;
  } else {
    line = "Automatic";
  }
  return (
    <div data-slot="routing-kind" data-open={open || undefined} className={cn(open && "bg-foreground/3")}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="hover:bg-foreground/3 flex w-full items-center gap-3 px-4 py-3 text-start"
      >
        <Icon aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-label font-medium">{PLAIN_KIND_NAMES[category]}</span>
          <span className="text-foreground/65 truncate text-xs">
            {line}
            {overrides > 0 && ` · ${overrides} area ${overrides === 1 ? "override" : "overrides"}`}
          </span>
        </span>
        <Chevron aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
      </button>
      {open && (
        <div className="flex flex-col gap-3 px-4 pt-1 pb-4 ps-11">
          <p className="text-muted-foreground text-xs">{KIND_HINTS[category]}</p>
          {children}
        </div>
      )}
    </div>
  );
}

/** Commit messages: the model Git settings name, when they name one, wins over this order. */
function GitWriterNote({ groups }: { groups: readonly ModelGroup[] }) {
  const gitWriter = useApp((s) => s.settings.gitWriter);
  if (!gitWriter) return null;
  return (
    <p className="text-foreground/65 text-xs">
      Git settings write commit messages with{" "}
      {shortModelName(groups, gitWriter.provider, gitWriter.model ?? "")}; the order below applies when they're
      set to Automatic.{" "}
      <button type="button" className="text-link hover:underline" onClick={() => openSettings("git")}>
        Open Git
      </button>
    </p>
  );
}
