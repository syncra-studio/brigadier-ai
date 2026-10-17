import { ChevronDown, ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import {
  faded,
  Profile,
  RATINGS_ROWS,
  RefreshButton,
  RefreshLines,
  strengthFor,
  tierFor,
} from "@/app/routing/ModelRatings";
import { ROUTED_KINDS } from "@/app/routing/words";
import { SettingsCard, SettingsSection, SettingsSelect } from "@/app/settings/parts";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Badge } from "@/components/ui/badge";
import type {
  Learned,
  MergedModel,
  RegistryInfo,
  RouteCandidate,
  RoutePreview,
  TaskCategory,
} from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { PLAIN_KIND_NAMES, TIER_LABELS, VENDOR_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { modelAvailable } from "@/state/providers";
import type { RankingsRefreshView } from "@/state/rankings";
import { useApp } from "@/state/store";

/**
 * Model strength: the models you use, ranked by how good each is at one kind of work, with a
 * bar and its score out of 10, faded below the kind's tier floor, and what routing picks now.
 * A row opens the model's profile. Under the list, the ratings' date and Refresh rankings.
 */
export function ModelStrength({
  models,
  learned,
  registry,
  rankings,
  category,
  onCategory,
  route,
  projectName,
  candidates,
  now,
}: {
  models: readonly MergedModel[] | null;
  learned: readonly Learned[];
  registry: RegistryInfo | null;
  rankings: RankingsRefreshView;
  category: TaskCategory;
  onCategory: (category: TaskCategory) => void;
  /** What routing picks for the kind now, for the Picked now mark. */
  route: RoutePreview | null;
  /** The project the learned numbers are for; `null`: every project. */
  projectName: string | null;
  /** The ranking's candidates, for each model's quota and load now. */
  candidates: readonly RouteCandidate[];
  now: number;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const availability = useApp((s) => s.settings);
  // Models switched off on Providers (or whose agent is) get no work, so they aren't ranked here.
  const ranked = (models ?? [])
    .filter((model) => !model.excluded && modelAvailable(availability, { provider: model.provider, id: model.id }))
    .flatMap((model) => {
      const strength = strengthFor(model, category);
      return strength === null ? [] : [{ model, strength }];
    })
    .toSorted((a, b) => b.strength - a.strength);
  // What each model is picked for now: a review asked without an author has a pick per vendor's
  // change, as it goes to the other vendor.
  const marks = new Map<string, string[]>();
  const mark = (provider: string, model: string | null, words: string) =>
    marks.set(`${provider}/${model}`, [...(marks.get(`${provider}/${model}`) ?? []), words]);
  if (route && route.byAuthor.length > 0) {
    for (const { author, outcome } of route.byAuthor) {
      if (outcome.type !== "chosen") continue;
      mark(outcome.choice.provider, outcome.choice.model, `Reviews ${VENDOR_LABELS[author]}'s changes`);
    }
  } else if (route?.outcome.type === "chosen") {
    mark(route.outcome.choice.provider, route.outcome.choice.model, "Picked now");
  }

  return (
    <SettingsSection
      title={RATINGS_ROWS.ratings.label}
      description={RATINGS_ROWS.ratings.description}
      actions={
        <SettingsSelect
          label="The kind of work the models are ranked for"
          value={category}
          options={ROUTED_KINDS.map((kind) => ({ value: kind, label: PLAIN_KIND_NAMES[kind] }))}
          onChange={onCategory}
        />
      }
    >
      <div data-setting={RATINGS_ROWS.ratings.label} data-slot="model-strength">
        <SettingsCard>
          {!models && <p className="text-muted-foreground px-4 py-3 text-xs">Reading the models…</p>}
          {ranked.map(({ model, strength }, index) => {
            const key = `${model.provider}/${model.id}`;
            const dim = faded(model, category);
            const isOpen = open === key;
            const Chevron = isOpen ? ChevronDown : ChevronRight;
            return (
              <div key={key} data-slot="strength-row" className={cn(isOpen && "bg-foreground/3")}>
                <button
                  type="button"
                  aria-expanded={isOpen}
                  onClick={() => setOpen(isOpen ? null : key)}
                  className="hover:bg-foreground/3 flex w-full items-center gap-3 px-4 py-2.5 text-start"
                >
                  <span className="text-muted-foreground w-4 shrink-0 font-mono text-xs">{index + 1}</span>
                  <ProviderGlyph provider={model.provider} className="size-icon-sm shrink-0" />
                  <span className="flex w-36 min-w-0 shrink-0 flex-col">
                    <span className={cn("truncate text-label font-medium", dim && "text-foreground/65")}>
                      {model.displayName}
                    </span>
                    <span className="text-muted-foreground text-2xs">{TIER_LABELS[tierFor(model, category)]}</span>
                  </span>
                  <span aria-hidden className="bg-foreground/8 h-1.5 min-w-0 flex-1 overflow-hidden rounded-full">
                    <span
                      className={cn("block h-full rounded-full", dim ? "bg-foreground/25" : "bg-foreground/70")}
                      style={{ width: `${Math.max(0, Math.min(10, strength)) * 10}%` }}
                    />
                  </span>
                  <span className={cn("w-8 shrink-0 text-end font-mono text-xs", dim && "text-foreground/45")}>
                    {strength.toFixed(1)}
                  </span>
                  <span className="flex min-w-24 shrink-0 justify-end gap-1">
                    {marks.get(key)?.map((words) => (
                      <Badge key={words} variant="success">
                        {words}
                      </Badge>
                    ))}
                  </span>
                  <Chevron aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
                </button>
                {isOpen && (
                  <Profile
                    model={model}
                    learned={learned}
                    registry={registry}
                    category={category}
                    projectName={projectName}
                    candidate={
                      candidates.find((found) => found.provider === model.provider && found.model === model.id) ?? null
                    }
                  />
                )}
              </div>
            );
          })}
          <div className="flex flex-col pt-1">
            <div className="text-muted-foreground flex flex-wrap items-center justify-between gap-3 px-4 py-2.5 text-xs">
              <span>
                {registry ? `Ratings updated ${registry.updated}` : "Ratings"}
                {registry?.checkedAtMs != null && ` · checked ${formatAgo(registry.checkedAtMs, now)}`}
              </span>
              <RefreshButton rankings={rankings} />
            </div>
            <RefreshLines rankings={rankings} models={models} registry={registry} />
          </div>
        </SettingsCard>
      </div>
    </SettingsSection>
  );
}
