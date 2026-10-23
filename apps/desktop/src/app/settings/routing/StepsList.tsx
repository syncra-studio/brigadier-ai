import { HAIRLINE_TOP, StepNumber } from "@/app/settings/routing/parts";
import { useListedModels } from "@/app/settings/routing/pickers";
import { refusingRule, rowChoice } from "@/app/settings/routing/ruleWords";
import { rowModelName, workRows } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared";
import type { ModelRules } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** How a new worker's model and effort are chosen, in the order it really happens (`brigadier start`). */
const STEPS: readonly [string, string][] = [
  ["Your choice", "If you or the thread name a model or effort for the worker, that is used."],
  ["Your order", "If not, the worker takes the first model in your order for its kind of work that your rules allow."],
  ["Automatic", "With no order of your own, it takes the first model of the automatic advice that your rules allow."],
  [
    "Your rules",
    "Last, your rules check the result: a model they keep out, or an effort above your cap, is refused and the worker doesn't start. Your rules win over everything above.",
  ],
];

/** A fixed example, when no kind of work on the page shows one. */
const EXAMPLE =
  "For example, if your rules keep the first model for Independent review out, the next model in its order runs; with none left, the worker is refused.";

/**
 * The example line: the first kind of work whose first model your rules keep out while a later
 * one may run, in the page's own words; else the fixed example.
 */
function useExample(rules: ModelRules | null, groups: readonly ModelGroup[]): string {
  const models = useListedModels();
  for (const row of rules ? workRows(rules) : []) {
    const refused = (index: number) => refusingRule(rules!.rules, rowChoice(row.rows[index]!, models), row.work) !== null;
    if (row.rows.length < 2 || !refused(0)) continue;
    const next = row.rows.findIndex((_, index) => !refused(index));
    if (next < 0) continue;
    const name = (index: number) => rowModelName(groups, row.rows[index]!.agent, row.rows[index]!.model);
    const from = row.mine.length > 0 ? "your order" : "the automatic advice";
    return `For example, ${row.work}: your rules keep ${name(0)} out, so ${name(next)}, next in ${from}, runs.`;
  }
  return EXAMPLE;
}

/** How a model is picked, opened: what decides a new worker's model, numbered, then an example. */
export function StepsList({ rules, groups }: { rules: ModelRules | null; groups: readonly ModelGroup[] }) {
  const example = useExample(rules, groups);
  return (
    <div data-slot="routing-steps" className="ps-7.5 pb-1">
      <ol className={cn(HAIRLINE_TOP, "flex flex-col pt-1.5 pb-2.5")}>
        {STEPS.map(([step, hint], index) => (
          <li key={step} className="flex gap-3.5 py-2.5">
            <StepNumber>{index + 1}</StepNumber>
            <span className="flex min-w-0 flex-1 flex-col gap-0.75 pt-0.5">
              <span className="text-label text-foreground">{step}</span>
              <span className="text-xs text-foreground/65">{hint}</span>
            </span>
          </li>
        ))}
      </ol>
      <p data-slot="routing-steps-example" className="text-xs text-muted-foreground pb-2.5">
        {example}
      </p>
    </div>
  );
}
