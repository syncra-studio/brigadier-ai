import { KindRow } from "@/app/settings/routing/KindRow";
import { workRows } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { type ModelGroup, SettingsCard } from "@/components/shared";
import type { ModelRules } from "@/ipc/generated";

export const WHO_DOES_WHAT = {
  label: "Who does what",
  description: "Automatic advice for each kind of work. Open one to set your own order of models and their effort.",
} as const;

/**
 * Who does what: a row per kind of work of the rules file with who picks and the model it
 * starts on; a row opens in place to Automatic (the shipped rows) or My order (your own).
 */
export function WhoDoesWhat({
  rules,
  groups,
  open,
  onOpen,
}: {
  rules: ModelRules | null;
  groups: readonly ModelGroup[];
  /** The kind of work open, by its name. */
  open: string | null;
  onOpen: (work: string | null) => void;
}) {
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
              onToggle={() => onOpen(open === row.work ? null : row.work)}
            />
          ))}
      </SettingsCard>
    </SettingsSection>
  );
}
