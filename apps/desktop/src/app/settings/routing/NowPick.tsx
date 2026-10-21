import { reviewers, shortModelName } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared";
import { ProviderGlyph } from "@/components/icons";
import type { RoutePreview, TaskCategory } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";

/** "Now ✻ Opus 5.5": what the kind runs on now; a chat's varies; a kind that waits says so. */
export function NowPick({
  category,
  route,
  groups,
  now,
}: {
  category: TaskCategory;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  now: number;
}) {
  // A chat's own model, or a review's for each vendor's change, varies.
  if (category === "chat" || reviewers(route)) {
    return <span className="text-xs text-muted-foreground shrink-0">Varies</span>;
  }
  if (!route) return null;
  const outcome = route.outcome;
  return (
    <span data-slot="now-pick" className="flex shrink-0 items-center gap-2">
      <span className="text-xs text-muted-foreground">Now</span>
      {outcome.type === "wait" ? (
        <span className="text-xs text-warning">
          Waits{outcome.resetsAtMs !== null && ` · ${formatCountdown(outcome.resetsAtMs, now)}`}
        </span>
      ) : (
        <span className="flex items-center gap-1.75">
          <ProviderGlyph provider={outcome.choice.provider} className="shrink-0 size-3.5" />
          <span className="text-xs text-foreground">
            {shortModelName(groups, outcome.choice.provider, outcome.choice.model ?? "")}
          </span>
        </span>
      )}
    </span>
  );
}
