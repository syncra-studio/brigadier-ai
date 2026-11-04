import { since } from "@/app/settings/routing/RatingsLine";
import { rowModelName } from "@/app/settings/routing/words";
import { type ModelGroup, RowText } from "@/components/shared";
import { Input } from "@/components/controls";
import { Telescope } from "@/components/icons";
import type { RankingsRefresh } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { setSetting } from "@/state/settings";

/** The research row's words, for the page and Settings search. */
export const RESEARCH_ROW = {
  label: "Daily ratings research",
  description: "Once a day, the best research model with quota left reads model cards and benchmarks and updates the ratings Automatic follows",
} as const;

/** What the latest research did, in a few words: "Last ran 3 hours ago on Sonnet 5.5 · 2 models re-rated". */
export function researchLine(refresh: RankingsRefresh | null, now: number, groups: readonly ModelGroup[] = []): string {
  if (!refresh?.startedAtMs) return "Hasn't run yet";
  const at = since(refresh.finishedAtMs ?? refresh.startedAtMs, now);
  const by = refresh.model ? ` on ${refresh.provider ? rowModelName(groups, refresh.provider, refresh.model) : refresh.model}` : "";
  switch (refresh.state) {
    case "checkingRegistry":
    case "researching":
      return `Researching now${by}…`;
    case "done":
      return `Last ran ${at}${by} · ${refresh.changes.length} ${refresh.changes.length === 1 ? "model" : "models"} re-rated`;
    case "failed":
      return `Last try ${at} didn't change the ratings${refresh.errors[0] ? `: ${refresh.errors[0]}` : ""}`;
    case "superseded":
      return `Last ran ${at}; a newer published registry has replaced it`;
    case "cancelled":
      return `Last try ${at} was stopped`;
    case "idle":
      return "Hasn't run yet";
  }
}

/**
 * Daily ratings research: when it runs (local time; a missed time runs at the next start) and
 * what it last did. Your own orders never change: better-rated models show as suggestions.
 */
export function ResearchRow({ refresh, now, groups }: { refresh: RankingsRefresh | null; now: number; groups: readonly ModelGroup[] }) {
  const at = useApp((s) => s.settings.ratingsResearchAt || "04:00");
  return (
    <div data-setting={RESEARCH_ROW.label} data-slot="ratings-research" className="flex items-center gap-3.5 py-3.5">
      <Telescope aria-hidden className="text-foreground/65 size-4 shrink-0" />
      <RowText truncate title={RESEARCH_ROW.label} line={researchLine(refresh, now, groups)} />
      <Input
        type="time"
        aria-label="Research time"
        className="w-28 shrink-0"
        value={at}
        onChange={(event) => {
          if (/^\d{2}:\d{2}$/.test(event.target.value)) void setSetting("ratingsResearchAt", event.target.value);
        }}
      />
    </div>
  );
}
