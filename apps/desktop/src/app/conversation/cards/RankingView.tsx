import type { ModelGroup } from "@/components/shared";
import { Badge } from "@/components/controls";
import { useNow } from "@/hooks/use-now";
import type { RankingUse } from "@/ipc/generated";
import { placeName, withResetTime } from "@/lib/routing";

/**
 * How the user's manual ranking took part, as it was when the model was chosen: the place that
 * ran it, or that none could, and every place passed over with why.
 */
export function RankingView({ ranking, groups }: { ranking: RankingUse; groups: readonly ModelGroup[] }) {
  const now = useNow(ranking.skipped.some((place) => place.resetsAtMs !== null) ? 60_000 : null);
  const text = ranking.text.charAt(0).toUpperCase() + ranking.text.slice(1);
  return (
    <div data-slot="route-ranking" className="flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-1.5">
        <Badge variant="secondary" title="Your manual ranking for this kind of work">
          Your ranking
        </Badge>
        <span className="text-xs">
          {ranking.position !== null
            ? `#${ranking.position} of ${ranking.places} in ${ranking.text}`
            : ranking.only
              ? `None of the models in ${ranking.text} could take it; it waits for them`
              : `None of the models in ${ranking.text} could take it, so routing scored the others`}
        </span>
      </div>
      {ranking.skipped.length > 0 && (
        <ul aria-label={`${text}: passed over`} className="flex flex-col gap-0.5 text-xs">
          {ranking.skipped.map((place) => (
            <li key={place.position} className="flex gap-2">
              <span className="shrink-0">
                #{place.position}{" "}
                {place.model && place.target.type !== "model"
                  ? `${placeName(place.target, groups)} (${place.model})`
                  : placeName(place.target, groups)}
              </span>
              <span className="text-muted-foreground min-w-0">
                {place.why !== null && withResetTime(place.why, place.resetsAtMs, now)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
