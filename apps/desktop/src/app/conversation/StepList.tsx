import type { FC } from "react";

import type { planStepStatus } from "@/app/conversation/planProgress";
import { StepGlyph } from "@/components/conversation";
import { cn } from "@/lib/utils";

/** A plan's whole checklist, as its pill shows it on hover: each step's mark and words. */
export const StepList: FC<{ steps: readonly { title: string; status: ReturnType<typeof planStepStatus> }[] }> = ({ steps }) => (
  <ol className="flex max-h-(--radix-tooltip-content-available-height) max-w-80 flex-col gap-2 overflow-y-auto px-0.5 py-1 text-start">
    {steps.map((step, index) => (
      <li key={index} className="flex min-w-0 items-start gap-2">
        <StepGlyph status={step.status} size="xs" />
        <span
          className={cn(
            "min-w-0 max-w-72 leading-4 wrap-break-word",
            step.status === "done" ? "text-muted-foreground/70" : step.status === "active" ? "text-foreground" : "text-muted-foreground",
          )}
        >
          {step.title}
        </span>
        <span className="sr-only">
          {step.status === "pending" ? "Not started" : step.status === "active" ? "Running" : step.status === "done" ? "Done" : step.status === "stopped" ? "Stopped" : "Failed"}
        </span>
      </li>
    ))}
  </ol>
);
