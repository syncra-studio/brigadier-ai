import { Info } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { More, MORE_ROWS, type MoreRow } from "@/app/routing/More";
import { RULES, Rules } from "@/app/routing/Rules";
import { WHO_DOES_WHAT, WhoDoesWhat } from "@/app/routing/WhoDoesWhat";
import { SettingsPage } from "@/app/settings/parts";
import { revealSetting, useReveal } from "@/app/settings/reveal";
import { useNow } from "@/hooks/use-now";
import type { Area, TaskCategory } from "@/ipc/generated";
import { useModelGroups } from "@/lib/setup";
import { useRankingsRefresh } from "@/state/rankings";
import { useRoutePreview } from "@/state/routing";
import { useUsageRefresh } from "@/state/usage";

/** The Routing page's rows, for Settings search; the page renders this copy. */
export const ROUTING_PAGE_ROWS = {
  kinds: WHO_DOES_WHAT,
  rules: RULES,
  strength: MORE_ROWS.strength,
  steps: MORE_ROWS.steps,
  refresh: { ...MORE_ROWS.refresh, target: MORE_ROWS.strength.label },
} as const;

const NO_AREAS: Area[] = [];

/**
 * Settings › Routing: who does what (a row per kind of work, which opens in place to Automatic or
 * your order), the rules, then More: the models' strength for a kind of work and how a model is
 * picked.
 */
export function RoutingPage() {
  useUsageRefresh();
  const groups = useModelGroups();
  const ratings = useRankingsRefresh();
  const now = useNow(30_000);
  const { routes, error } = useRoutePreview(null, NO_AREAS);
  const [more, setMore] = useState<MoreRow>(null);
  const [kind, setKind] = useState<TaskCategory>("review");
  // Settings search bringing one of More's rows into view opens it.
  const revealing = useReveal((s) => s.label);
  const asked =
    revealing === MORE_ROWS.strength.label || revealing === MORE_ROWS.refresh.label
      ? "strength"
      : revealing === MORE_ROWS.steps.label
        ? "steps"
        : null;
  if (asked && more !== asked) setMore(asked);
  const seeStrengths = (category: TaskCategory) => {
    setKind(category);
    setMore("strength");
    revealSetting(MORE_ROWS.strength.label);
  };
  return (
    <SettingsPage
      title="Routing"
      description="Which model takes each kind of work. Brigadier picks automatically; change only what you want to."
      actions={
        <span className="text-settings-small text-settings-text-tertiary flex items-center gap-1.5 pt-2.25">
          <Info aria-hidden className="size-3.25" />
          Applies to new work only
        </span>
      }
    >
      {error && (
        <p role="alert" className="text-destructive text-xs">
          {error}
        </p>
      )}
      <WhoDoesWhat routes={routes} groups={groups} now={now} onSeeStrengths={seeStrengths} />
      <Rules routes={routes} groups={groups} />
      <More
        open={more}
        onOpen={setMore}
        category={kind}
        onCategory={setKind}
        route={routes?.find((found) => found.category === kind) ?? null}
        rankings={ratings}
        now={now}
      />
    </SettingsPage>
  );
}
