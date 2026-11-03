import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { RatingsLine, RefreshModels } from "@/app/settings/routing/RatingsLine";
import { More, MORE_ROWS, type MoreRow } from "@/app/settings/routing/More";
import { RULES, Rules } from "@/app/settings/routing/Rules";
import { WHO_DOES_WHAT, WhoDoesWhat } from "@/app/settings/routing/WhoDoesWhat";
import { SettingsPage } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { useNow } from "@/hooks/use-now";
import { request } from "@/ipc/client";
import type { TaskCategory } from "@/ipc/generated";
import { useModelGroups } from "@/lib/setup";
import { refreshProviders } from "@/state/actions";
import { loadModelRules, useModelRules, useModelRulesRead } from "@/state/modelRules";
import { refreshUsage, useUsage, useUsageRefresh } from "@/state/usage";

/** The Routing page's rows, for Settings search; the page renders this copy. */
export const ROUTING_PAGE_ROWS = {
  kinds: WHO_DOES_WHAT,
  rules: RULES,
  research: MORE_ROWS.research,
  strength: MORE_ROWS.strength,
  steps: MORE_ROWS.steps,
  refresh: MORE_ROWS.refresh,
} as const;

/**
 * Settings › Routing, the editor of the user's rules file (`~/.brigadier/models.md`): who does
 * what (a row per kind of work, which opens in place to the shipped rows or your own order), the
 * rules (everywhere or for one kind of work), then More: the models' strength for a kind of work
 * and how a model is picked.
 */
export function RoutingPage() {
  useUsageRefresh();
  useModelRulesRead();
  const groups = useModelGroups();
  const rules = useModelRules((s) => s.view);
  const error = useModelRules((s) => s.error);
  const now = useNow(30_000);
  const refreshing = useAction();
  const registry = useUsage((s) => s.view?.registry ?? null);
  // Accounts that didn't answer in time, from the usage check Refresh runs.
  const usageError = useUsage((s) => s.refreshError);
  const [more, setMore] = useState<MoreRow>(null);
  const [kind, setKind] = useState<TaskCategory>("review");
  // The kind of work open in Who does what.
  const [work, setWork] = useState<string | null>(null);
  // Settings search bringing one of More's rows into view opens it.
  const revealing = useReveal((s) => s.label);
  const asked =
    revealing === MORE_ROWS.strength.label
      ? "strength"
      : revealing === MORE_ROWS.steps.label
        ? "steps"
        : null;
  if (asked && more !== asked) setMore(asked);
  return (
    <SettingsPage
      title="Routing"
      description={
        <>
          Which agent, model and effort suit each kind of work. Changes apply to new work only.
          <RatingsLine registry={registry} now={now} />
        </>
      }
      actions={
        <RefreshModels
          label={MORE_ROWS.refresh.label}
          busy={refreshing.busy}
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
      }
    >
      {(refreshing.error ?? usageError) && (
        <p role="alert" className="text-destructive text-xs">
          {refreshing.error ?? usageError}
        </p>
      )}
      {error && (
        <p role="alert" className="text-destructive text-xs">
          {error}
        </p>
      )}
      <WhoDoesWhat rules={rules} groups={groups} open={work} onOpen={setWork} />
      <Rules rules={rules} groups={groups} />
      <More
        open={more}
        onOpen={setMore}
        category={kind}
        onCategory={setKind}
        rules={rules}
        groups={groups}
        now={now}
      />
    </SettingsPage>
  );
}
