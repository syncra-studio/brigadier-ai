import { ModelEffort } from "@/app/settings/routing/ModelEffort";
import { Badge, HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { useListedModels } from "@/app/settings/routing/pickers";
import { refusingRule, rowAvailable, rowChoice } from "@/app/settings/routing/ruleWords";
import { rowModelName, type WorkRow } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared";
import type { ModelRule } from "@/ipc/generated";
import { joinWords } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

/**
 * Automatic: the kind's rows in order (each agent's best-rated model, best first; the shipped
 * rows where nothing is rated), the one picked now marked and the next named, each with its
 * effort; then those your rules keep out, and the first row's reason.
 */
export function AutoPicks({ row, rules, groups }: { row: WorkRow; rules: readonly ModelRule[]; groups: readonly ModelGroup[] }) {
  const models = useListedModels();
  const availability = useApp((s) => s.settings);
  const name = (found: WorkRow["shipped"][number]) => rowModelName(groups, found.agent, found.model);
  const refused = (found: WorkRow["shipped"][number]) => refusingRule(rules, rowChoice(found, models), row.work);
  const eligible = row.shipped.filter((found) => !refused(found));
  const skipped = row.shipped.filter((found) => refused(found));
  const picked = eligible.findIndex((found) => rowAvailable(found, models, availability));
  return (
    <div data-slot="auto-picks" className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-3")}>
      {eligible.map((found, index) => (
        <div key={`${found.agent}/${found.model}`} data-slot="auto-pick" className="flex h-10 items-center gap-3">
          <span className="text-xs text-muted-foreground w-2 shrink-0 font-mono">{index + 1}</span>
          <span className="flex min-w-0 flex-1 items-center">
            <ModelEffort row={found} groups={groups} />
          </span>
          {index === picked ? (
            <Badge tone="success">Picked now</Badge>
          ) : (
            picked >= 0 &&
            index === picked + 1 && (
              <span className="text-xs text-muted-foreground truncate">Next if {name(eligible[picked]!)} can't run</span>
            )
          )}
        </div>
      ))}
      {skipped.length > 0 && (
        <p className="text-xs text-muted-foreground pt-1">
          Skipped: {joinWords(skipped.map(name))} {skipped.length === 1 ? "is" : "are"} kept out by your rules. Choose My order to
          pick another model.
        </p>
      )}
      {row.shipped[0] && <p className="text-xs text-muted-foreground pt-1">{row.shipped[0].reason}</p>}
    </div>
  );
}
