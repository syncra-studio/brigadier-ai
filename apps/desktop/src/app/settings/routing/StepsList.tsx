import { HAIRLINE_TOP, StepNumber } from "@/app/settings/routing/parts";
import { cn } from "@/lib/utils";

/** The steps a worker's agent, model and effort are settled by, in order. */
const STEPS: readonly [string, string][] = [
  ["Your choice", "An agent, model or effort the thread names for a worker is used as given"],
  ["Your order", "Otherwise the first model of your order for its kind of work that your rules allow"],
  ["Automatic", "Otherwise the shipped advice for that kind of work, the same way"],
  ["Your rules", "Last, a choice a rule forbids, everywhere or for that work, is refused, quoting the rule"],
];

/** How a model is picked, opened: the steps a worker's model is settled by, numbered. */
export function StepsList() {
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
    </div>
  );
}
