import { useState } from "react";

import { More, MORE_ROWS, type MoreRow } from "@/app/settings/routing/More";
import { RULES, Rules } from "@/app/settings/routing/Rules";
import { WHO_DOES_WHAT, WhoDoesWhat } from "@/app/settings/routing/WhoDoesWhat";
import { SettingsPage } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { Info } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { TaskCategory } from "@/ipc/generated";
import { useModelGroups } from "@/lib/setup";
import { useModelRules, useModelRulesRead } from "@/state/modelRules";
import { useUsageRefresh } from "@/state/usage";

/** The Routing page's rows, for Settings search; the page renders this copy. */
export const ROUTING_PAGE_ROWS = {
  kinds: WHO_DOES_WHAT,
  rules: RULES,
  strength: MORE_ROWS.strength,
  steps: MORE_ROWS.steps,
  refresh: { ...MORE_ROWS.refresh, target: MORE_ROWS.strength.label },
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
  const [more, setMore] = useState<MoreRow>(null);
  const [kind, setKind] = useState<TaskCategory>("review");
  // The kind of work open in Who does what; a rule's note opens one it would stop from starting.
  const [work, setWork] = useState<string | null>(null);
  const openWork = (name: string) => {
    setWork(name);
    requestAnimationFrame(() =>
      document.querySelector(`[data-slot="routing-kind"][data-work="${CSS.escape(name)}"]`)?.scrollIntoView({ block: "nearest" }),
    );
  };
  // Settings search bringing one of More's rows into view opens it.
  const revealing = useReveal((s) => s.label);
  const asked =
    revealing === MORE_ROWS.strength.label || revealing === MORE_ROWS.refresh.label
      ? "strength"
      : revealing === MORE_ROWS.steps.label
        ? "steps"
        : null;
  if (asked && more !== asked) setMore(asked);
  return (
    <SettingsPage
      title="Routing"
      description="Which agent, model and effort suit each kind of work. The thread follows these when it starts workers; change only what you want to."
      actions={
        <span className="text-xs text-muted-foreground flex items-center gap-1.5 pt-2.25">
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
      <WhoDoesWhat rules={rules} groups={groups} open={work} onOpen={setWork} />
      <Rules rules={rules} groups={groups} onOpenKind={openWork} />
      <More
        open={more}
        onOpen={setMore}
        category={kind}
        onCategory={setKind}
        now={now}
        rules={rules}
        groups={groups}
      />
    </SettingsPage>
  );
}
