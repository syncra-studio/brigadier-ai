import { type Step, STEP_LABELS, STEPS, closeSetup } from "@/app/setup/onboarding/steps";
import { Button } from "@/components/controls";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/overlays";
import { X } from "@/components/icons";
import { cn } from "@/lib/utils";

/**
 * The strip along the top: the window drags by it, a segment per step (the current one longer
 * and bright; steps already reached can be revisited) and, on a run from Settings, Close at the
 * end.
 */
export function OnboardingTopBar({
  step,
  reached,
  onStep,
  closable,
}: {
  step: Step;
  reached: number;
  onStep: (step: Step) => void;
  closable: boolean;
}) {
  const current = STEPS.indexOf(step);
  return (
    <div
      data-tauri-drag-region
      className="h-titlebar relative z-10 grid shrink-0 grid-cols-[1fr_auto_1fr] items-center px-2"
    >
      <TooltipProvider>
        <nav aria-label="Setup steps" className="col-start-2 flex items-center gap-1">
          {STEPS.map((entry, index) => {
            const active = index === current;
            return (
              <Tooltip key={entry}>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    aria-label={`Step ${index + 1}: ${STEP_LABELS[entry]}`}
                    aria-current={active ? "step" : undefined}
                    disabled={index > reached}
                    onClick={() => onStep(entry)}
                    className={cn(
                      "rounded-capsule relative h-0.75 transition-all before:absolute before:-inset-x-0.5 before:-inset-y-2",
                      active ? "bg-foreground w-7.5" : "bg-muted enabled:hover:bg-muted-foreground/45 w-3.75",
                    )}
                  />
                </TooltipTrigger>
                <TooltipContent side="bottom">{STEP_LABELS[entry]}</TooltipContent>
              </Tooltip>
            );
          })}
        </nav>
      </TooltipProvider>
      {closable && (
        <Button
          size="icon-sm"
          variant="ghost"
          aria-label="Close setup"
          onClick={closeSetup}
          // As designed: round, 28 across, 20 in from the window's edge.
          className="text-muted-foreground hover:text-foreground me-3 size-7 justify-self-end rounded-full"
        >
          <X className="size-3.75" />
        </Button>
      )}
    </div>
  );
}
