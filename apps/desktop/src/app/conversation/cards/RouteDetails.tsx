import { Section } from "@/app/conversation/cards/common";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import type { ModelGroup } from "@/components/shared/model-selector";
import { Badge } from "@/components/controls/badge";
import { useNow } from "@/hooks/use-now";
import type { Attempt, Explanation, RankingUse, Task } from "@/ipc/generated";
import { formatDateTime, formatTime } from "@/lib/format";
import {
  choiceName,
  formatDelta,
  formatScore,
  hasResetCountdown,
  limitPhrase,
  placeName,
  routeReason,
  TIER_LABELS,
  withResetTime,
} from "@/lib/routing";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * Why a worker runs on its model, in its details: the explanation behind the choice and every
 * model that ran it. None of it goes into the conversation's thread.
 */

/** The explanation behind a choice: its factors, the runners-up and what decided it. */
export function ExplanationView({
  explanation,
  groups,
}: {
  explanation: Explanation;
  groups: readonly ModelGroup[];
}) {
  const { score, factors, alternatives, rule, trial, balancing, ranking } = explanation;
  return (
    <div data-slot="route-explanation" className="flex flex-col gap-2">
      {ranking && <RankingView ranking={ranking} groups={groups} />}
      {(trial || balancing || rule) && (
        <div className="flex flex-wrap items-center gap-1.5">
          {trial && (
            <Badge variant="warning" title="An unrated model trying out low-risk work">
              Trial
            </Badge>
          )}
          {balancing && (
            <Badge variant="secondary" title="Quota balancing moved this work off a provider running hot">
              Balancing
            </Badge>
          )}
          {rule && <span className="text-xs">Your rule decided it: {rule}</span>}
        </div>
      )}
      {factors.length > 0 && (
        <table className="w-full text-xs">
          <caption className="sr-only">Score</caption>
          <tbody>
            {factors.map((factor, index) => (
              <tr key={index}>
                <td className="py-0.5 pe-3">{factor.label}</td>
                <td className="text-muted-foreground w-0 py-0.5 text-end tabular-nums whitespace-nowrap">
                  {formatDelta(factor.delta)}
                </td>
              </tr>
            ))}
            {score !== null && (
              <tr className="border-t">
                <td className="py-0.5 pe-3 font-medium">Score</td>
                <td className="w-0 py-0.5 text-end font-medium tabular-nums">{formatScore(score)}</td>
              </tr>
            )}
          </tbody>
        </table>
      )}
      {alternatives.length > 0 && (
        <div className="flex flex-col gap-0.5">
          <p className="text-muted-foreground text-xs">Also considered</p>
          <ul className="flex flex-col gap-0.5 text-xs">
            {alternatives.map((alternative) => (
              <li key={`${alternative.provider}/${alternative.model}`} className="flex gap-2">
                <span className="shrink-0">
                  {choiceName(groups, {
                    provider: alternative.provider,
                    model: alternative.model,
                    effort: null,
                  })}
                  {alternative.score !== null && (
                    <span className="text-muted-foreground tabular-nums">
                      {" "}
                      ({formatScore(alternative.score)})
                    </span>
                  )}
                </span>
                <span className="text-muted-foreground min-w-0">{alternative.whyNot}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/**
 * How the user's manual ranking took part, as it was when the model was chosen: the place that
 * ran it, or that none could, and every place passed over with why.
 */
function RankingView({ ranking, groups }: { ranking: RankingUse; groups: readonly ModelGroup[] }) {
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

function attemptEnd(
  attempt: Attempt,
  providers: Parameters<typeof limitPhrase>[2],
  now: number,
): string | null {
  const end = attempt.end;
  if (!end) return null;
  return end.type === "limit"
    ? `It ${limitPhrase(end.limit, attempt.route.choice.provider, providers, now)}.`
    : `It failed: ${end.message}`;
}

/** Every model that ran the task, oldest first, with how each run ended. */
export function AttemptsView({
  attempts,
  groups,
}: {
  attempts: readonly Attempt[];
  groups: readonly ModelGroup[];
}) {
  const providers = useApp((s) => s.providers.view?.providers);
  const now = useNow(60_000);
  return (
    <ol data-slot="route-attempts" className="flex flex-col gap-1.5">
      {attempts.map((attempt, index) => {
        const end = attemptEnd(attempt, providers, now);
        const current = index === attempts.length - 1 && attempt.endedAtMs === null;
        return (
          <li key={attempt.startedAtMs} className="flex flex-col text-xs">
            <span>
              {choiceName(groups, attempt.route.choice)}
              {attempt.route.choice.effort && ` · ${attempt.route.choice.effort}`}
              <span className="text-muted-foreground">
                {" "}
                · {formatDateTime(attempt.startedAtMs)}
                {attempt.endedAtMs !== null
                  ? `–${formatTime(attempt.endedAtMs)}`
                  : current
                    ? " · now"
                    : ""}
              </span>
            </span>
            <span className="text-muted-foreground truncate" title={routeReason(attempt.route, now)}>
              {routeReason(attempt.route, now)}
            </span>
            {end && <span className="text-warning">{end}</span>}
          </li>
        );
      })}
    </ol>
  );
}

/** The details' routing sections: why this model (with its explanation) and its attempts. */
export function RouteSections({
  task,
  model,
  groups,
  inThread,
}: {
  task: Task;
  model: string;
  groups: readonly ModelGroup[];
  inThread: boolean;
}) {
  const { route } = task;
  const now = useNow(hasResetCountdown(route.reason) ? 60_000 : null);
  return (
    <>
      <Section title="Why this model">
        {!inThread && (
          <p className="text-sm">
            {PROVIDER_LABELS[route.choice.provider]} · {model}
          </p>
        )}
        <p className="text-muted-foreground text-xs">{routeReason(route, now, task.quotaWait?.resetsAtMs ?? null)}</p>
        <p className="text-muted-foreground text-xs">
          Quality floor: {TIER_LABELS[task.floor]}
          {task.areas.length > 0 && ` · touches ${task.areas.join(", ")}`}
        </p>
        {route.explanation && <ExplanationView explanation={route.explanation} groups={groups} />}
        <p className="text-muted-foreground text-xs">
          You choose which models get each kind of work in{" "}
          <button
            type="button"
            data-slot="routing-hint"
            onClick={() => openSettings("routing")}
            className="text-link rounded-control hover:underline"
          >
            Settings › Routing
          </button>
        </p>
      </Section>
      {(task.attempts.length > 1 ||
        task.attempts.some((attempt) => attempt.end !== null) ||
        task.reworkRounds > 0) && (
        <Section title="Models that ran it">
          <AttemptsView attempts={task.attempts} groups={groups} />
          {task.reworkRounds > 0 && (
            <p className="text-muted-foreground text-xs">
              Sent back to rework{" "}
              {task.reworkRounds === 1 ? "once" : `${task.reworkRounds} times`} after reporting
            </p>
          )}
        </Section>
      )}
    </>
  );
}
