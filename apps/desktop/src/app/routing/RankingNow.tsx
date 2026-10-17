import { Check, ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { ChipSelect, useScopeOptions } from "@/app/routing/pickers";
import { formatPart, scoreParts, shortModelName, spacing } from "@/app/routing/words";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Badge } from "@/components/ui/badge";
import { useNow } from "@/hooks/use-now";
import type { Area, ProviderKind, RouteCandidate, RoutePreview, TaskCategory } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { AREA_LABELS, AREAS, TIER_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";

/** Blocked models shown before "N more". */
const SHOWN_BLOCKED = 4;

/** The table's columns: place, model, the five parts, score, effort and its badge. */
const COLUMNS = {
  gridTemplateColumns: `${spacing(6)} minmax(0,1fr) repeat(5,${spacing(15)}) ${spacing(14)} ${spacing(14)} ${spacing(22)}`,
};

/** The preview the table is for: the project, the area a task touches and who wrote a change. */
export type RankingQuestion = {
  projectId: string | null;
  area: Area | null;
  author: ProviderKind | null;
};

const AREA_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "no area" },
  ...AREAS.map((area) => ({ value: area, label: AREA_LABELS[area] })),
];

const AUTHOR_OPTIONS: { value: string; label: string }[] = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
  { value: "", label: "no one" },
];

/**
 * Ranking right now: every model routing weighs for the next task of a kind, in the order it
 * would try them, with what each score is made of; the one it picks first; then those that can't
 * take it, with why.
 */
export function RankingNow({
  category,
  route,
  question,
  onQuestion,
  groups,
}: {
  category: TaskCategory;
  route: RoutePreview | null;
  question: RankingQuestion;
  onQuestion: (question: RankingQuestion) => void;
  groups: readonly ModelGroup[];
}) {
  const scopes = useScopeOptions();
  const [all, setAll] = useState(false);
  const now = useNow(30_000);
  const candidates = route?.candidates ?? [];
  const eligible = candidates.filter((candidate) => candidate.blocked === null);
  const blocked = candidates.filter((candidate) => candidate.blocked !== null);
  const shownBlocked = all ? blocked : blocked.slice(0, SHOWN_BLOCKED);
  const reason = route?.outcome.type === "chosen" ? route.outcome.reason : null;

  return (
    <section data-setting="Ranking right now" data-slot="ranking-now" className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-medium">Ranking right now</h3>
        <span className="text-muted-foreground flex flex-wrap items-center gap-1.5 text-xs">
          Scored for
          <ChipSelect
            label="The project the ranking is scored for"
            value={question.projectId ?? ""}
            options={scopes}
            onChange={(value) => onQuestion({ ...question, projectId: value === "" ? null : value })}
          />
          ·
          <ChipSelect
            label="The area the task touches"
            value={question.area ?? ""}
            options={AREA_OPTIONS}
            onChange={(value) => onQuestion({ ...question, area: value === "" ? null : (value as Area) })}
          />
          {category === "review" && (
            <>
              · written by
              <ChipSelect
                label="Who wrote the change under review"
                value={question.author ?? ""}
                options={AUTHOR_OPTIONS}
                onChange={(value) =>
                  onQuestion({ ...question, author: value === "" ? null : (value as ProviderKind) })
                }
              />
            </>
          )}
        </span>
      </div>
      <div className="bg-card border-divider rounded-settings overflow-x-auto border text-xs">
        <div style={{ minWidth: spacing(192) }}>
          <div className="text-muted-foreground border-divider grid items-center gap-2 border-b px-4 py-2" style={COLUMNS}>
            <span>#</span>
            <span>Model</span>
            <span className="text-end">Strength</span>
            <span className="text-end">Area</span>
            <span className="text-end">Learned</span>
            <span className="text-end">Quota</span>
            <span className="text-end">Load</span>
            <span className="text-end">Score</span>
            <span className="text-center">Effort</span>
            <span />
          </div>
          {!route && <p className="text-muted-foreground px-4 py-3">Reading what routing would pick…</p>}
          {route?.outcome.type === "wait" && (
            <p className="text-warning border-divider border-b px-4 py-2">
              The next task waits
              {route.outcome.resetsAtMs !== null && ` · ${formatCountdown(route.outcome.resetsAtMs, now)}`}:{" "}
              {route.outcome.reason}
            </p>
          )}
          {eligible.map((candidate, index) => {
            const place = route?.places.find(
              (found) => found.target.provider === candidate.provider && found.model === candidate.model,
            );
            const skipped = !candidate.chosen && place?.why ? place.why : null;
            return (
              <EligibleRow
                key={`${candidate.provider}/${candidate.model}`}
                candidate={candidate}
                position={skipped ? null : index + 1}
                note={candidate.chosen ? reason : skipped}
                skipped={skipped !== null}
                groups={groups}
              />
            );
          })}
          {shownBlocked.map((candidate) => (
            <div
              key={`${candidate.provider}/${candidate.model}`}
              data-slot="blocked-candidate"
              className="border-divider flex items-center justify-between gap-4 border-b px-4 py-2 ps-12"
            >
              <span className="text-foreground/65 flex min-w-0 items-center gap-2">
                <ProviderGlyph provider={candidate.provider} className="size-icon-xs shrink-0" />
                <span className="truncate">{shortModelName(groups, candidate.provider, candidate.model)}</span>
              </span>
              <span className="text-muted-foreground min-w-0 truncate text-end" title={candidate.blocked ?? undefined}>
                {candidate.blocked}
                {candidate.resetsAtMs !== null && ` · ${formatCountdown(candidate.resetsAtMs, now)} left`}
              </span>
            </div>
          ))}
          <div className="flex items-center justify-between gap-4 px-4 py-2.5 ps-12">
            {blocked.length > SHOWN_BLOCKED ? (
              <button
                type="button"
                className="text-link flex items-center gap-1 hover:underline"
                onClick={() => setAll(!all)}
              >
                <ChevronRight
                  aria-hidden
                  className={cn("size-icon-xs transition-transform motion-reduce:transition-none", all && "rotate-90")}
                />
                {all ? "Show fewer" : `${blocked.length - SHOWN_BLOCKED} more that can't take it`}
              </button>
            ) : (
              <span />
            )}
            <span className="text-muted-foreground font-mono text-2xs">
              score = strength + area + learned − quota − load + trial
            </span>
          </div>
        </div>
      </div>
      {route?.trialSlot && (
        <p className="text-muted-foreground text-xs">The next task of this kind may try out a new model.</p>
      )}
    </section>
  );
}

function EligibleRow({
  candidate,
  position,
  note,
  skipped,
  groups,
}: {
  candidate: RouteCandidate;
  /** Its place in the order; `null` when the list passed over it. */
  position: number | null;
  note: string | null;
  skipped: boolean;
  groups: readonly ModelGroup[];
}) {
  const parts = scoreParts(candidate);
  return (
    <div
      data-slot="route-candidate"
      data-chosen={candidate.chosen || undefined}
      className={cn("border-divider border-b px-4 py-2.5", candidate.chosen && "bg-foreground/4")}
    >
      <div className="grid items-center gap-2" style={COLUMNS}>
        <span className="text-muted-foreground font-mono">{position ?? "–"}</span>
        <span className="flex min-w-0 items-center gap-2">
          <ProviderGlyph provider={candidate.provider} className="size-icon-sm shrink-0" />
          <span className="truncate text-sm">{shortModelName(groups, candidate.provider, candidate.model)}</span>
          <span className="text-muted-foreground shrink-0">{TIER_LABELS[candidate.tier]}</span>
        </span>
        <span className="text-end font-mono">{parts.strength === null ? "—" : parts.strength.toFixed(1)}</span>
        <span className="text-end font-mono">{formatPart(parts.area)}</span>
        <span className="text-end font-mono">{formatPart(parts.learned)}</span>
        <span className={cn("text-end font-mono", parts.quota < 0 && "text-warning")}>{formatPart(parts.quota)}</span>
        <span className="text-end font-mono">{formatPart(parts.load)}</span>
        <span className="text-end font-mono font-medium">
          {candidate.score === null ? "—" : candidate.score.toFixed(2)}
        </span>
        <span className="text-center font-mono">{candidate.effort ?? "—"}</span>
        <span className="flex justify-end">
          {candidate.chosen ? (
            <Badge variant="success">
              <Check />
              Picked
            </Badge>
          ) : skipped ? (
            <Badge variant="secondary">Skipped</Badge>
          ) : candidate.trial ? (
            <Badge variant="warning">Trial</Badge>
          ) : null}
        </span>
      </div>
      {note && <p className="text-foreground/65 ps-8 pt-1">{note}</p>}
    </div>
  );
}
