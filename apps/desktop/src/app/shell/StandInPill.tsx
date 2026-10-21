import type { ModelGroup } from "@/components/shared";
import { Shuffle } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { useNow } from "@/hooks/use-now";
import type { ModelFallback } from "@/ipc/generated";
import { choiceName, formatResetAt, VENDOR_LABELS } from "@/lib/routing";
import { openSettings } from "@/state/actions";

/**
 * The model standing in while the chosen one is at a limit: "On Codex gpt-6-sol until 21:10 ·
 * Claude limit", the reason on hover. The picker keeps showing the saved choice.
 */
export function StandInPill({
  fallback,
  groups,
}: {
  fallback: ModelFallback;
  groups: readonly ModelGroup[];
}) {
  const now = useNow(60_000);
  const until = fallback.untilMs !== null ? ` until ${formatResetAt(fallback.untilMs, now)}` : "";
  const text = `On ${choiceName(groups, fallback.choice)}${until} · ${VENDOR_LABELS[fallback.replaces.provider]} limit`;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          data-slot="stand-in-model"
          aria-label={`${text}. ${fallback.reason} Open Providers.`}
          onClick={() => openSettings("providers")}
          className="h-pill px-pill-x rounded-capsule bg-warning/15 text-warning hover:bg-warning/25 inline-flex max-w-sm min-w-0 shrink items-center gap-1 text-xs transition-colors"
        >
          <Shuffle aria-hidden className="size-icon-xs shrink-0" />
          {/* A narrow composer keeps the icon; the rest is on hover. */}
          <span className="@lg/composer:inline hidden truncate">{text}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs flex-col items-start gap-0.5">
        <span className="font-medium">{text}</span>
        <span>{fallback.reason}</span>
        <span className="text-muted-foreground text-xs">
          Your choice, {choiceName(groups, fallback.replaces)}, takes over again
          {fallback.untilMs !== null ? ` at ${formatResetAt(fallback.untilMs, now)}` : " when its limit resets"}.
        </span>
      </TooltipContent>
    </Tooltip>
  );
}
