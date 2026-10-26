import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { type ProviderTransition, transitionAccountLabel } from "@/app/conversation/providerTransitions";
import { LabeledDivider } from "@/components/shared";
import { Info } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { formatDateTime } from "@/lib/format";
import { modelName, useModelGroups } from "@/lib/setup";
import { useBoard } from "@/state/board";

/** Completed recovery, from provider acceptance; never a speculative picker selection. */
export function ProviderTransitionDivider({ transition }: { transition: ProviderTransition }) {
  const groups = useModelGroups();
  const worker = useBoard((s) => transition.taskId ? s.board?.tasks[transition.taskId]?.title ?? "Worker" : null);
  const action = transition.cause === "automaticFallback" ? "Switched automatically to"
    : transition.cause === "manualSwitch" ? "Switched to" : "Usage available · Resumed on";
  const label = transitionAccountLabel(transition);
  return (
    <LabeledDivider data-slot="provider-transition" data-transition-id={transition.id} role="status">
      <span className="min-w-0 max-w-full text-center wrap-break-word">
        {worker && `${worker} · `}{action} {PROVIDER_LABELS[transition.to.provider]}
        {` · ${modelName(groups, transition.to)}`}
        {label && ` · ${label}`}
      </span>
      <Tooltip>
        <TooltipTrigger asChild>
          <button type="button" aria-label="Provider transition details" className="hover:text-foreground shrink-0">
            <Info aria-hidden className="size-icon-sm" />
          </button>
        </TooltipTrigger>
        <TooltipContent className="flex-col items-start gap-0.5">
          <span>Provider: {PROVIDER_LABELS[transition.account.provider]}</span>
          <span>Account: {transition.account.account ?? "Own account"}</span>
          {label && <span>Name: {label}</span>}
          <span>Started: {formatDateTime(transition.startedAtMs)}</span>
        </TooltipContent>
      </Tooltip>
    </LabeledDivider>
  );
}
