import type { FC } from "react";

import { Info } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import type { ModelChoice } from "@/ipc/generated";
import type { ProviderTransition } from "@/app/conversation/providerTransitions";
import { modelName, sameModel, useModelGroups } from "@/lib/setup";

/** The line shown when a turn ran on another model than the one picked (a fallback). */
export const ModelChanged: FC<{ model: ModelChoice | null; picked: ModelChoice | null; transitions?: readonly ProviderTransition[] }> = ({
  model,
  picked,
  transitions = [],
}) => {
  const groups = useModelGroups();
  if (!model || !picked || sameModel(groups, picked, model)) return null;
  if (transitions.some((transition) => sameModel(groups, transition.to, model)
    || (sameModel(groups, transition.from, model) && sameModel(groups, transition.to, picked)))) return null;
  return (
    <p data-slot="model-changed" className="text-muted-foreground flex items-center gap-1.5 text-sm">
      Model changed from {modelName(groups, picked)} to {modelName(groups, model)}.
      <Tooltip>
        <TooltipTrigger asChild>
          <button type="button" aria-label="Why the model changed" className="hover:text-foreground shrink-0">
            <Info aria-hidden className="size-icon-sm" />
          </button>
        </TooltipTrigger>
        <TooltipContent>The model you picked couldn’t take this turn, so another one answered it.</TooltipContent>
      </Tooltip>
    </p>
  );
};
