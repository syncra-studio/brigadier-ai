import { useState } from "react";

import { KindRow } from "@/app/settings/routing/KindRow";
import { ROUTED_KINDS } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { type ModelGroup, SettingsCard } from "@/components/shared";
import type { RoutePreview, TaskCategory } from "@/ipc/generated";

export const WHO_DOES_WHAT = {
  label: "Who does what",
  description: "Automatic picks the strongest model that can run right now. Open a kind of work to set your own order.",
} as const;

/**
 * Who does what: a row per kind of work with how it picks and what it runs on now; a row opens
 * in place to Automatic (the models it would try) or My order (the order you set).
 */
export function WhoDoesWhat({
  routes,
  groups,
  now,
  onSeeStrengths,
}: {
  routes: RoutePreview[] | null;
  groups: readonly ModelGroup[];
  now: number;
  /** Opens Model strength on a kind of work. */
  onSeeStrengths: (category: TaskCategory) => void;
}) {
  const [open, setOpen] = useState<TaskCategory | null>(null);
  return (
    <SettingsSection title={WHO_DOES_WHAT.label} description={WHO_DOES_WHAT.description}>
      <SettingsCard inset data-slot="who-does-what">
        {ROUTED_KINDS.map((category) => (
          <KindRow
            key={category}
            category={category}
            open={open === category}
            route={routes?.find((found) => found.category === category) ?? null}
            groups={groups}
            now={now}
            onToggle={() => setOpen(open === category ? null : category)}
            onSeeStrengths={() => onSeeStrengths(category)}
          />
        ))}
      </SettingsCard>
    </SettingsSection>
  );
}
