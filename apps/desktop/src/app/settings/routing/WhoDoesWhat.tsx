import { useState } from "react";

import { KindRow } from "@/app/settings/routing/KindRow";
import { workRows } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { type ModelGroup, SettingsCard } from "@/components/shared";
import type { ModelRules } from "@/ipc/generated";

export const WHO_DOES_WHAT = {
  label: "Who does what",
  description: "Shipped advice for each kind of work. Open one to set your own agent, model and effort.",
} as const;

/**
 * Who does what: a row per kind of work of the rules file with whose row it follows and the
 * model it names; a row opens in place to Shipped (the shipped row) or Mine (your own).
 */
export function WhoDoesWhat({ rules, groups }: { rules: ModelRules | null; groups: readonly ModelGroup[] }) {
  const [open, setOpen] = useState<string | null>(null);
  return (
    <SettingsSection title={WHO_DOES_WHAT.label} description={WHO_DOES_WHAT.description}>
      <SettingsCard inset data-slot="who-does-what">
        {rules &&
          workRows(rules).map((row) => (
            <KindRow
              key={row.work}
              row={row}
              rules={rules}
              open={open === row.work}
              groups={groups}
              onToggle={() => setOpen(open === row.work ? null : row.work)}
            />
          ))}
      </SettingsCard>
    </SettingsSection>
  );
}
