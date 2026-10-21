import { HAIRLINE_TOP, StepNumber } from "@/app/settings/routing/parts";
import { cn } from "@/lib/utils";

/** The steps routing narrows the models by, in order. */
const STEPS: readonly [string, string][] = [
  ["Hard limits", "Skips models that are signed out, at a limit, just failed or can't read images"],
  ["Never rules", "Skips models your rules keep away from this work"],
  ["Tier floor", "Skips models below the tier this work needs, e.g. Frontier for code, review and merge"],
  ["Your order", "If you set one, the first model that can run takes it. Skips the next two steps"],
  ["Prefer rules", "Moves models your rules prefer to the front"],
  ["Orchestrator pin", "Coordinate work stays with the model that started it"],
  ["Best score", "Otherwise the highest of strength + learned − quota − load"],
];

/** How a model is picked, opened: the steps routing narrows the models by, numbered. */
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
