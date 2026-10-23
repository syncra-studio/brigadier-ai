import { useAction } from "@/app/conversation/useAction";
import { KindOfWorkRow } from "@/app/settings/routing/KindOfWorkRow";
import { LoadingLine } from "@/app/settings/routing/LoadingLine";
import { MoreRowHead } from "@/app/settings/routing/MoreRowHead";
import { RatingsFooter } from "@/app/settings/routing/RatingsFooter";
import { StepsList } from "@/app/settings/routing/StepsList";
import { StrengthRow } from "@/app/settings/routing/StrengthRow";
import { rankedFor, strengthFor } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { ListOrderedIcon, SpeedometerLatencySpeed } from "@/components/icons";
import { type ModelGroup, SettingsCard } from "@/components/shared";
import { request } from "@/ipc/client";
import type { ModelRules, TaskCategory } from "@/ipc/generated";
import { refreshProviders } from "@/state/actions";
import { loadModelRules } from "@/state/modelRules";
import { modelAvailable } from "@/state/providers";
import { useApp } from "@/state/store";
import { refreshUsage, useUsage } from "@/state/usage";

/** More's rows, for the page and Settings search. */
export const MORE_ROWS = {
  strength: { label: "Model strength", description: "How good each model is at a kind of work, out of 10" },
  steps: {
    label: "How a model is picked",
    description: "What decides the model a new worker runs on, and what wins",
  },
  refresh: {
    label: "Refresh models",
    description:
      "Asks each agent for the models it offers, checks every account's usage and reset times, and reads the latest ratings and automatic advice again, so the advice the thread reads is up to date. Your own rows and rules stay as they are.",
  },
} as const;

/** Which of More's rows is open. */
export type MoreRow = "strength" | "steps" | null;

/**
 * More: Model strength (the models ranked for one kind of work, with Refresh models) and How a
 * model is picked, each opening in place.
 */
export function More({
  open,
  onOpen,
  category,
  onCategory,
  now,
  rules,
  groups,
}: {
  open: MoreRow;
  onOpen: (row: MoreRow) => void;
  /** The kind of work the models are ranked for. */
  category: TaskCategory;
  onCategory: (category: TaskCategory) => void;
  now: number;
  /** The rules file, for How a model is picked's example. */
  rules: ModelRules | null;
  groups: readonly ModelGroup[];
}) {
  const view = useUsage((s) => s.view);
  const availability = useApp((s) => s.settings);
  const refreshing = useAction();
  // Accounts that didn't answer in time, from the usage check Refresh runs.
  const usageError = useUsage((s) => s.refreshError);
  // Models switched off on Providers (or whose agent is) aren't ranked here.
  const rated = (view?.models ?? [])
    .filter((model) => !model.excluded && modelAvailable(availability, { provider: model.provider, id: model.id }))
    .flatMap((model) => {
      const strength = strengthFor(model, category);
      return strength === null ? [] : [{ model, strength }];
    });
  const ranked = rankedFor(rated, category);
  const registry = view?.registry ?? null;
  const toggle = (row: Exclude<MoreRow, null>) => onOpen(open === row ? null : row);
  return (
    <SettingsSection title="More">
      <SettingsCard inset data-slot="routing-more">
        <MoreRowHead
          row={MORE_ROWS.strength}
          icon={<SpeedometerLatencySpeed aria-hidden />}
          open={open === "strength"}
          summary={ranked[0] && `${ranked[0].model.displayName} leads`}
          onToggle={() => toggle("strength")}
        >
          <div data-slot="model-strength" className="flex flex-col ps-7.5">
            <KindOfWorkRow category={category} onCategory={onCategory} />
            {!view && <LoadingLine />}
            {ranked.map(({ model, strength }, index) => (
              <StrengthRow
                key={`${model.provider}/${model.id}`}
                model={model}
                strength={strength}
                place={index + 1}
                category={category}
              />
            ))}
            <RatingsFooter
              label={MORE_ROWS.refresh.label}
              registry={registry}
              now={now}
              running={refreshing.busy}
              busy={refreshing.busy}
              disabled={refreshing.busy}
              onRefresh={() =>
                refreshing.run(async () => {
                  // The live state `brigadier models` shows (each agent's models, every
                  // account's usage), the ratings, then the automatic advice and your file.
                  await Promise.all([refreshProviders(null), request({ method: "checkRegistry" })]);
                  await refreshUsage();
                  await loadModelRules();
                })
              }
            />
            {(refreshing.error ?? usageError) && (
              <p role="alert" className="text-destructive text-xs pb-3">
                {refreshing.error ?? usageError}
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
          <StepsList rules={rules} groups={groups} />
        </MoreRowHead>
      </SettingsCard>
    </SettingsSection>
  );
}
