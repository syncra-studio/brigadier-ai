import { Badge, HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { reviewers, reviewsWords, shortModelName } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared";
import { Button } from "@/components/controls";
import { ProviderGlyph } from "@/components/icons";
import type { RoutePreview, TaskCategory } from "@/ipc/generated";
import { DEFAULT_FLOORS, joinWords, TIER_LABELS, tierBelow } from "@/lib/routing";
import { cn } from "@/lib/utils";

/**
 * Automatic: the models that can take the work, in the order routing would try them, the one
 * picked now marked and the next named; then those skipped as below the tier the work needs.
 */
export function AutoPicks({
  category,
  route,
  groups,
  onSeeStrengths,
}: {
  category: TaskCategory;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  onSeeStrengths: () => void;
}) {
  const candidates = route?.candidates ?? [];
  const eligible = candidates.filter((candidate) => candidate.blocked === null);
  const floor = DEFAULT_FLOORS[category];
  const skipped = candidates.filter((candidate) => candidate.blocked !== null && tierBelow(candidate.tier, floor));
  const name = (provider: (typeof candidates)[number]["provider"], model: string) => shortModelName(groups, provider, model);
  // A review picked for each vendor's change marks whose changes each model reviews instead.
  const byAuthor = reviewers(route);
  const picked = byAuthor ? undefined : eligible.find((candidate) => candidate.chosen);
  return (
    <div data-slot="auto-picks" className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-3")}>
      {eligible.map((candidate, index) => (
        <div key={`${candidate.provider}/${candidate.model}`} data-slot="auto-pick" className="flex h-10 items-center gap-3">
          <span className="text-xs text-muted-foreground w-2 shrink-0 font-mono">{index + 1}</span>
          <span className="flex min-w-0 flex-1 items-center gap-2">
            <ProviderGlyph provider={candidate.provider} className="shrink-0 size-3.5" />
            <span className="text-label text-foreground truncate">{name(candidate.provider, candidate.model)}</span>
          </span>
          {byAuthor ? (
            byAuthor.has(`${candidate.provider}/${candidate.model}`) && (
              <Badge tone="success">{reviewsWords(byAuthor.get(`${candidate.provider}/${candidate.model}`)!)}</Badge>
            )
          ) : candidate.chosen ? (
            <Badge tone="success">Picked now</Badge>
          ) : (
            picked &&
            index === 1 && (
              <span className="text-xs text-muted-foreground truncate">
                Next if {name(picked.provider, picked.model)} can't run
              </span>
            )
          )}
          <span className="text-xs text-foreground w-7.5 shrink-0 text-end font-mono font-medium">
            {candidate.score?.toFixed(1)}
          </span>
        </div>
      ))}
      {skipped.length > 0 && (
        <p className="text-xs text-muted-foreground pt-1">
          Skipped: {joinWords(skipped.map((candidate) => name(candidate.provider, candidate.model)))}{" "}
          {skipped.length === 1 ? "is" : "are"} below the {TIER_LABELS[floor]} tier this work needs.{" "}
          <Button variant="link-blue" size="inline" className="inline-flex" onClick={onSeeStrengths}>See strengths</Button>
        </p>
      )}
    </div>
  );
}
