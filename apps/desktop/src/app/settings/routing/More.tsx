import { KindOfWorkRow } from "@/app/settings/routing/KindOfWorkRow";
import { LoadingLine } from "@/app/settings/routing/LoadingLine";
import { MoreRowHead } from "@/app/settings/routing/MoreRowHead";
import { RatingsFooter } from "@/app/settings/routing/RatingsFooter";
import { refreshRunning } from "@/app/settings/routing/rankings";
import { StepsList } from "@/app/settings/routing/StepsList";
import { StrengthRow } from "@/app/settings/routing/StrengthRow";
import { reviewers, strengthFor } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { ListOrderedIcon, SpeedometerLatencySpeed } from "@/components/icons";
import { SettingsCard } from "@/components/shared";
import type { RoutePreview, TaskCategory } from "@/ipc/generated";
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

/** Which of More's rows is open. */
export type MoreRow = "strength" | "steps" | null;

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
                picked={picked}
                byAuthor={byAuthor}
              />
            ))}
            <RatingsFooter
              label={MORE_ROWS.refresh.label}
              registry={registry}
              now={now}
              running={running}
              busy={starting.busy}
              disabled={running || starting.busy || refresh === null}
              onRefresh={() => starting.run(start)}
            />
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
          <StepsList />
        </MoreRowHead>
      </SettingsCard>
    </SettingsSection>
  );
}
