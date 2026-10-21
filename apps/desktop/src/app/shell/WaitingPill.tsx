import { Clock } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { useNow } from "@/hooks/use-now";
import type { QuotaWait } from "@/ipc/generated";
import { formatResetAt, withResetTime } from "@/lib/routing";
import { openSettings } from "@/state/actions";

/**
 * Messages waiting for quota: the model is at its limit and no model the user allows can stand
 * in. "Waiting for quota · 21:10", why on hover. They go on their own; Stop drops them.
 */
export function WaitingPill({ wait }: { wait: QuotaWait }) {
  const now = useNow(60_000);
  const until = wait.resetsAtMs !== null ? ` · ${formatResetAt(wait.resetsAtMs, now)}` : "";
  const text = `Waiting for quota${until}`;
  const reason = withResetTime(wait.reason, wait.resetsAtMs, now);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          data-slot="quota-wait-pill"
          aria-label={`${text}. ${reason} Open Routing settings.`}
          onClick={() => openSettings("routing")}
          className="h-pill px-pill-x rounded-capsule bg-warning/15 text-warning hover:bg-warning/25 inline-flex max-w-sm min-w-0 shrink items-center gap-1 text-xs transition-colors"
        >
          <Clock aria-hidden className="size-icon-xs shrink-0" />
          {/* A narrow composer keeps the icon; the rest is on hover. */}
          <span className="@lg/composer:inline hidden truncate">{text}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs flex-col items-start gap-0.5">
        <span className="font-medium">{text}</span>
        <span>{reason}</span>
        {wait.rule && <span>Your rule keeps it from other models: {wait.rule}</span>}
        {wait.ranking && <span>Kept for the models in {wait.ranking}.</span>}
        <span className="text-muted-foreground text-xs">
          Your messages go on their own once a model can take them; Stop drops them.
        </span>
      </TooltipContent>
    </Tooltip>
  );
}
