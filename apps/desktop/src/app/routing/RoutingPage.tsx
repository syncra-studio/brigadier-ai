import { Clock, InfoCircle, Undo } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { AREA_OVERRIDES_ROW, AreaOverrides } from "@/app/routing/AreaOverrides";
import { ModelRatings, RATINGS_ROWS } from "@/app/routing/ModelRatings";
import { useScopeOptions } from "@/app/routing/pickers";
import { RankingNow, type RankingQuestion } from "@/app/routing/RankingNow";
import { ruleCovers, RoutingRules } from "@/app/routing/RoutingRules";
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
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
} from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Badge } from "@/components/ui/badge";
import { useNow } from "@/hooks/use-now";
import type { Area, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { isNoWorkRule, KIND_HINTS, PLAIN_KIND_NAMES, ruleSentence, WORKER_CATEGORIES } from "@/lib/routing";
import { useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { openSettings } from "@/state/actions";
import { useRankingsRefresh } from "@/state/rankings";
import { changeRanking, useRoutePreview } from "@/state/routing";
import { useApp } from "@/state/store";
import { setUsageProject, useUsage, useUsageRefresh } from "@/state/usage";

/** The Routing page's rows, for Settings search; the page renders this copy. */
export const ROUTING_PAGE_ROWS = {
  kinds: {
    label: "Kinds of work",
    description: "Which model takes each kind of work Brigadier hands out, and in what order",
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
    target: "Kinds of work",
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
 * Settings › Routing: the kinds of work, each with the models it tries in order (or Automatic),
 * and how routing picks; the chosen kind's order, its ranking right now, its rules and its area
 * overrides; then every model's ratings.
 */
export function RoutingPage() {
  useUsageRefresh();
  const groups = useModelGroups();
  const projects = useApp((s) => s.projects);
  const rules = useApp((s) => s.settings.routingOverrides);
  const rankings = useApp((s) => s.settings.routingRankings);
  const view = useUsage((s) => s.view);
  const ratings = useRankingsRefresh();
  const now = useNow(30_000);
  const scopes = useScopeOptions();
  const [kind, setKind] = useState<TaskCategory>("scout");
  const [scope, setScope] = useState<string | null>(null);
  const [question, setQuestion] = useState<RankingQuestion>({ projectId: null, area: null, author: "claude" });
  const { routes, error } = useRoutePreview(scope, NO_AREAS);
  const ranked = useRoutePreview(question.projectId, question.area ? [question.area] : NO_AREAS, question.author);

  // The learned numbers follow the project the ranking is scored for.
  useEffect(() => {
    if (useUsage.getState().projectId !== question.projectId) setUsageProject(question.projectId);
  }, [question.projectId]);

  // A rule Settings brings into view shows under the kind of work it is about.
  const revealing = useReveal((s) => s.label);
  const revealed = revealing ? rules.find((found) => ruleSentence(found, groups, projects) === revealing) : undefined;
  if (revealed && !ruleCovers(revealed, kind)) {
    const about = isNoWorkRule(revealed) ? WORKER_CATEGORIES[0] : revealed.categories[0];
    if (about) setKind(about);
  }

  const changeScope = (next: string | null) => {
    setScope(next);
    setQuestion((current) => ({ ...current, projectId: next }));
  };
  const own = rowRanking(rankings, kind, scope);
  const everywhere = scope === null ? null : rowRanking(rankings, kind, null);
  const scopeName = scope ? (projects[scope]?.name ?? "this project") : null;

  return (
    <SettingsPage
      title="Routing"
      description="Which model takes each kind of work, in what order, and what it's never given."
      actions={
        <span className="text-muted-foreground flex items-center gap-1.5 text-xs">
          <InfoCircle aria-hidden className="size-icon-sm" />
          Changes apply to new work, not work already running
        </span>
      }
    >
      <div className="-mt-3 flex flex-col gap-6">
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        <div
          data-slot="routing-kinds"
          className="bg-card/40 border-divider rounded-settings flex min-h-0 overflow-hidden border"
        >
          <aside className="border-divider flex w-72 shrink-0 flex-col gap-6 border-e p-5">
            <section data-setting={ROUTING_PAGE_ROWS.kinds.label} className="flex flex-col gap-0.5">
              <h2 className="text-muted-foreground px-2 pb-1 text-xs font-medium">{ROUTING_PAGE_ROWS.kinds.label}</h2>
              <div role="listbox" aria-label={ROUTING_PAGE_ROWS.kinds.label} className="flex flex-col gap-0.5">
                {ROUTED_KINDS.map((category) => (
                  <KindItem
                    key={category}
                    category={category}
                    selected={category === kind}
                    route={routeOf(category, routes)}
                    scope={scope}
                    rankings={rankings}
                    groups={groups}
                    now={now}
                    onSelect={() => setKind(category)}
                  />
                ))}
              </div>
            </section>
          </aside>
          <div className="flex min-w-0 flex-1 flex-col gap-7 p-6">
            <KindHead category={kind} scope={scope} rankings={rankings} />
            {scopeName && (
              <p className="text-foreground/65 -mt-3 text-xs">
                In {scopeName}: other projects follow All projects. Change which under Advanced.
              </p>
            )}
            {kind === "gitText" && <GitWriterNote groups={groups} />}
            <TryOrder
              category={kind}
              scope={scope}
              own={own}
              everywhere={everywhere}
              route={routeOf(kind, routes)}
              groups={groups}
            />
            <RoutingRules category={kind} scope={scope} groups={groups} />
          </div>
        </div>
      </div>
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

/** One kind of work in the list: its name and what it runs on now. */
function KindItem({
  category,
  selected,
  route,
  scope,
  rankings,
  groups,
  now,
  onSelect,
}: {
  category: TaskCategory;
  selected: boolean;
  route: RoutePreview | null;
  scope: string | null;
  rankings: readonly Ranking[];
  groups: readonly ModelGroup[];
  now: number;
  onSelect: () => void;
}) {
  const gitWriter = useApp((s) => s.settings.gitWriter);
  const Icon = KIND_ICONS[category];
  const own = rowRanking(rankings, category, scope);
  // A project without a row of its own follows All projects'; one set to Automatic doesn't.
  const followsAll = scope !== null && own === null;
  const list = decides(own) ? own : followsAll ? rowRanking(rankings, category, null) : null;
  const overrides = areaOverrides(rankings, category, scope).length;
  let line: React.ReactNode;
  if (category === "gitText" && gitWriter) {
    line = `→ ${shortModelName(groups, gitWriter.provider, gitWriter.model ?? "")}, set in Git`;
  } else if (route?.outcome.type === "wait") {
    line = (
      <span className="text-warning flex items-center gap-1">
        <Clock aria-hidden className="size-icon-xs" />
        Waits{route.outcome.resetsAtMs !== null && ` · ${formatCountdown(route.outcome.resetsAtMs, now)}`}
      </span>
    );
  } else if (decides(list)) {
    line = `→ ${orderSummary(list, groups)}`;
  } else if (category === "chat") {
    line = "→ the chat's model, then Automatic";
  } else if (authorPicks(route, groups, { effort: false })) {
    line = `→ Automatic · adversarial: ${authorPicks(route, groups, { effort: false })}`;
  } else if (route?.outcome.type === "chosen") {
    line = `→ Automatic · ${shortModelName(groups, route.outcome.choice.provider, route.outcome.choice.model ?? "")} now`;
  } else {
    line = "→ Automatic";
  }
  return (
    <button
      type="button"
      role="option"
      aria-selected={selected}
      data-slot="routing-kind"
      onClick={onSelect}
      className={cn(
        "rounded-nav flex gap-2.5 px-2 py-2 text-start",
        selected ? "bg-foreground/8" : "hover:bg-foreground/4",
      )}
    >
      <Icon aria-hidden className="text-foreground/65 size-icon-md mt-0.5 shrink-0" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="flex items-center gap-2 text-sm">
          {PLAIN_KIND_NAMES[category]}
          {overrides > 0 && (
            <Badge variant="secondary" className="font-normal">
              {overrides} {overrides === 1 ? "override" : "overrides"}
            </Badge>
          )}
        </span>
        <span className="text-muted-foreground truncate text-xs">{line}</span>
      </span>
    </button>
  );
}

/** The chosen kind's icon, name and what it is, with Reset to default. */
function KindHead({
  category,
  scope,
  rankings,
}: {
  category: TaskCategory;
  scope: string | null;
  rankings: readonly Ranking[];
}) {
  const action = useAction();
  const Icon = KIND_ICONS[category];
  // What Reset turns back to Automatic: the kind's order and its area overrides here.
  const mine = rankings.filter(
    (ranking) => ranking.category === category && ranking.projectId === scope && ranking.manual,
  );
  const reset = () =>
    action.run(async () => {
      for (const ranking of mine) await changeRanking(ranking.id, (current) => ({ ...current, manual: false }));
    });
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 gap-3">
          <span className="bg-foreground/8 rounded-nav flex size-9 shrink-0 items-center justify-center">
            <Icon aria-hidden className="size-icon-md" />
          </span>
          <div className="flex min-w-0 flex-col gap-0.5">
            <h2 className="text-xl font-medium">{PLAIN_KIND_NAMES[category]}</h2>
            <p className="text-foreground/65 text-xs">{KIND_HINTS[category]}</p>
          </div>
        </div>
        <SettingsButton
          disabled={mine.length === 0 || action.busy}
          title="Back to Automatic here; your orders are kept"
          onClick={reset}
        >
          <Undo />
          Reset to default
        </SettingsButton>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </div>
  );
}

/** Commit messages: the model Git settings name, when they name one, wins over this order. */
function GitWriterNote({ groups }: { groups: readonly ModelGroup[] }) {
  const gitWriter = useApp((s) => s.settings.gitWriter);
  if (!gitWriter) return null;
  return (
    <p className="text-foreground/65 -mt-3 text-xs">
      Git settings write commit messages with{" "}
      {shortModelName(groups, gitWriter.provider, gitWriter.model ?? "")}; the order below applies when they're
      set to Automatic.{" "}
      <button type="button" className="text-link hover:underline" onClick={() => openSettings("git")}>
        Open Git
      </button>
    </p>
  );
}
