import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useMemo } from "react";

import { useAction } from "@/app/conversation/useAction";
import { learnedKey, modelSummary, ModelRow, ProjectPicker } from "@/app/routing/models";
import {
  RANKINGS_ROWS,
  RatingsSection,
  RefreshRankingsButton,
  RefreshStatusLine,
} from "@/app/routing/RankingsSection";
import { AdvancedRouting, ROUTING_ROWS, RoutingKinds } from "@/app/routing/routing";
import {
  SettingsAdvanced,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSwitch,
} from "@/app/settings/parts";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Badge } from "@/components/ui/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { useNow } from "@/hooks/use-now";
import type { Learned, MergedModel, ModelInfo, ProviderKind } from "@/ipc/generated";
import { isFable } from "@/lib/routing";
import { useAvailableModelGroups, useModelGroups } from "@/lib/setup";
import { openSettings } from "@/state/actions";
import { isNewModel, mayWork, modelAvailable, setModelWorks } from "@/state/providers";
import { useRankingsRefresh } from "@/state/rankings";
import { useApp } from "@/state/store";
import { useUsage, useUsageRefresh } from "@/state/usage";

/** The Routing page's rows, for Settings search; the page renders this copy. */
export const ROUTING_PAGE_ROWS = {
  workers: {
    label: "Models Brigadier can give work to",
    description:
      "Only models turned on here get tasks. This doesn't change the model you pick for a chat or session.",
  },
  refresh: RANKINGS_ROWS.refresh,
  whoDoesWhat: ROUTING_ROWS.simple,
  advanced: {
    label: "Advanced",
    description:
      "The full order and why, backup models, settings per project or area, rules like “never use this model for reviews”, each model's scores, and where the ratings come from.",
  },
  kinds: ROUTING_ROWS.kinds,
  rules: ROUTING_ROWS.rules,
  details: {
    label: "Model details",
    description:
      "Brigadier scores each model 0–10 per kind of work from the model registry, then adjusts the scores from how its tasks went.",
  },
  ratings: RANKINGS_ROWS.ratings,
  registry: {
    label: "Model registry",
    description: "The published list the models' ratings start from.",
  },
} as const;

/**
 * The Routing page: which available models the orchestrator may hand tasks to (a switch
 * each), then who does each kind of work (Automatic or a model). The full order, rules, the
 * models' scores and the registry wait under Advanced.
 */
export function RoutingPage() {
  useUsageRefresh();
  const view = useUsage((s) => s.view);
  const picked = useUsage((s) => s.projectId);
  // Names of every model (a rule may name one that isn't available); the lists offer only
  // the available ones.
  const groups = useModelGroups();
  const available = useAvailableModelGroups();
  const settings = useApp((s) => s.settings);
  const now = useNow(30_000);
  const rankings = useRankingsRefresh();
  // Adjustments read for another project than the one picked are not shown while it is read.
  const forPicked = view?.projectId === picked;
  const learned = useMemo(
    () =>
      new Map<string, Learned>(
        forPicked && view
          ? view.learned.map((entry) => [learnedKey(entry.provider, entry.model, entry.category), entry])
          : [],
      ),
    [view, forPicked],
  );
  const merged = view?.models ?? null;
  const detailed = (merged ?? []).filter((model) =>
    modelAvailable(settings, { provider: model.provider, id: model.id }),
  );
  const shown = available.filter((group) => group.models.length > 0);

  return (
    <SettingsPage
      wide
      title="Routing"
      description="In a session, Brigadier splits the work into tasks and hands each to a worker model. Choose which models it may use, and which one does each kind of work."
    >
      <SettingsSection
        title={ROUTING_PAGE_ROWS.workers.label}
        description={ROUTING_PAGE_ROWS.workers.description}
        actions={<RefreshRankingsButton rankings={rankings} />}
      >
        <RefreshStatusLine rankings={rankings} merged={merged} />
        <div className="flex flex-col gap-4 pt-2">
          {shown.length === 0 ? (
            <p className="text-muted-foreground text-xs">No model is available yet.</p>
          ) : (
            shown.map((group) => <WorkerGroup key={group.provider} group={group} merged={merged} />)
          )}
          <p className="text-muted-foreground flex flex-wrap items-center gap-x-2 gap-y-1 px-1 text-xs">
            Only models available on Providers show here.
            <button
              type="button"
              onClick={() => openSettings("providers")}
              className="text-link hover:underline"
            >
              Open Providers
            </button>
          </p>
        </div>
      </SettingsSection>

      <RoutingKinds groups={groups} />

      <SettingsAdvanced description={ROUTING_PAGE_ROWS.advanced.description}>
        <AdvancedRouting groups={groups} />
        <SettingsSection
          title={ROUTING_PAGE_ROWS.details.label}
          description={ROUTING_PAGE_ROWS.details.description}
        >
          <SettingsCard>
            <SettingsRow
              label="Learned in"
              description="The project the details show the adjustments for, or all of them."
            >
              <ProjectPicker projectId={picked} />
            </SettingsRow>
          </SettingsCard>
          {merged === null ? (
            <p className="text-muted-foreground text-xs">Reading the models…</p>
          ) : (
            <SettingsCard>
              {detailed.map((model) => (
                <ModelRow
                  key={`${model.provider}/${model.id}`}
                  model={model}
                  learned={learned}
                  projectId={picked}
                  groups={groups}
                />
              ))}
            </SettingsCard>
          )}
        </SettingsSection>
        <RatingsSection rankings={rankings} merged={merged} registry={view?.registry ?? null} now={now} />
      </SettingsAdvanced>
    </SettingsPage>
  );
}

/** One agent's available models, each with its switch; older models folded away. */
function WorkerGroup({ group, merged }: { group: ModelGroup; merged: readonly MergedModel[] | null }) {
  const current = group.models.filter((model) => !model.legacy);
  const older = group.models.filter((model) => model.legacy);
  return (
    <div className="flex flex-col gap-1.5">
      <h3 className="text-foreground/80 flex items-center gap-1.5 px-1 text-xs font-medium">
        <ProviderGlyph provider={group.provider} className="size-icon-sm shrink-0" />
        {group.label}
      </h3>
      <SettingsCard>
        {current.map((model) => (
          <WorkerRow key={model.id} provider={group.provider} model={model} merged={merged} />
        ))}
        {older.length > 0 && (
          <Collapsible>
            <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex w-full items-center gap-1.5 px-4 py-2.5 text-xs">
              <ChevronRight
                aria-hidden
                className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
              />
              {older.length} older {older.length === 1 ? "model" : "models"}
            </CollapsibleTrigger>
            <CollapsibleContent className="flex flex-col">
              {older.map((model) => (
                <WorkerRow key={model.id} provider={group.provider} model={model} merged={merged} />
              ))}
            </CollapsibleContent>
          </Collapsible>
        )}
      </SettingsCard>
    </div>
  );
}

/**
 * A model: its name, what it's best for, and whether Brigadier may give it tasks. Fable models
 * never get tasks (one of Brigadier's own rules), so their switch stays off.
 */
function WorkerRow({
  provider,
  model,
  merged,
}: {
  provider: ProviderKind;
  model: ModelInfo;
  merged: readonly MergedModel[] | null;
}) {
  const ref = { provider, id: model.id };
  const rules = useApp((s) => s.settings.routingOverrides);
  const toggle = useAction();
  const fable = isFable(model);
  const works = !fable && mayWork(rules, ref);
  const fresh = !fable && isNewModel(rules, ref);
  const known = merged?.find((entry) => entry.provider === provider && entry.id === model.id);
  const summary = fable
    ? "Brigadier never gives work to Fable models. You can still pick one for a chat or session."
    : fresh
      ? "New: Brigadier gives it no tasks until you turn it on."
      : modelSummary(model, known);
  return (
    <SettingsRow
      label={
        <span className="flex items-center gap-2">
          {model.displayName}
          {fresh && <Badge variant="warning">New</Badge>}
        </span>
      }
      description={summary || undefined}
      error={toggle.error}
    >
      <SettingsSwitch
        label={`Brigadier may give work to ${model.displayName}`}
        checked={works}
        disabled={fable || toggle.busy}
        onCheckedChange={(next) => toggle.run(() => setModelWorks(ref, next))}
      />
    </SettingsRow>
  );
}
