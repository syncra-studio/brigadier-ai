import { Clock, InfoCircle, Undo } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { AreaOverrides } from "@/app/routing/AreaOverrides";
import { ModelRatings, RATINGS_ROWS } from "@/app/routing/ModelRatings";
import { ChipSelect, useScopeOptions } from "@/app/routing/pickers";
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
import { SettingsButton, SettingsPage } from "@/app/settings/parts";
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
    description: "Hard limits, rules, tier floors, your order, then the score",
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
  overrides: {
    label: "Area overrides",
    description: "A different order for work that touches frontend, backend, infra, docs or tests",
  },
  ratings: RATINGS_ROWS.ratings,
  refresh: { ...RATINGS_ROWS.refresh, target: RATINGS_ROWS.ratings.label },
} as const;

/** The steps routing narrows the models by, in order. */
const STEPS: readonly [string, string | null][] = [
  ["Hard limits", "logged out, at a limit, just failed, can't read images"],
  ["Never · only rules", null],
  ["Tier floor", "Frontier for code, review, merge"],
  ["Manual list", "skips the floor"],
  ["Prefer rules", null],
  ["Orchestrator pin", null],
  ["Score", "strength + learned − quota − load"],
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
      wide
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
            <div className="bg-divider h-px" />
            <HowPicked />
          </aside>
          <div className="flex min-w-0 flex-1 flex-col gap-7 p-6">
            <KindHead category={kind} scope={scope} rankings={rankings} />
            <p className="text-foreground/65 -mt-3 flex flex-wrap items-center gap-2 text-sm">
              In
              <ChipSelect
                label="Where these settings apply"
                value={scope ?? ""}
                options={scopes}
                onChange={(value) => changeScope(value === "" ? null : value)}
              />
              — applies {scopeName ? `in ${scopeName}` : "everywhere"} unless an area override says otherwise.
            </p>
            {kind === "gitText" && <GitWriterNote groups={groups} />}
            <TryOrder
              category={kind}
              scope={scope}
              own={own}
              everywhere={everywhere}
              route={routeOf(kind, routes)}
              groups={groups}
            />
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
              groups={groups}
            />
            <RoutingRules category={kind} scope={scope} groups={groups} />
            <AreaOverrides category={kind} scope={scope} route={routeOf(kind, routes)} groups={groups} />
          </div>
        </div>
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
      </div>
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

/** The steps routing narrows the models by. */
function HowPicked() {
  return (
    <section data-setting={ROUTING_PAGE_ROWS.steps.label} className="flex flex-col gap-1 px-2">
      <h2 className="text-sm font-medium">{ROUTING_PAGE_ROWS.steps.label}</h2>
      <p className="text-muted-foreground text-xs">Each step narrows what the next one sees.</p>
      <ol className="flex flex-col gap-2 pt-2">
        {STEPS.map(([name, hint], index) => (
          <li key={name} className="flex gap-2.5">
            <span
              className={cn(
                "flex size-5 shrink-0 items-center justify-center rounded-xs font-mono text-2xs",
                index === STEPS.length - 1 ? "bg-foreground text-background" : "bg-foreground/8 text-foreground/65",
              )}
            >
              {index + 1}
            </span>
            <span className="flex flex-col">
              <span className="text-label">{name}</span>
              {hint && <span className="text-muted-foreground text-2xs">{hint}</span>}
            </span>
          </li>
        ))}
      </ol>
    </section>
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
