import { type ContextPoint, formatTokens } from "@/app/inspector/orchestratorLog";
import { UsageBar } from "@/components/shared";

export function ContextMeter({
  point,
  injected,
}: {
  point: ContextPoint | null;
  injected: number;
}) {
  const used = point?.usedTokens ?? null;
  const windowTokens = point?.windowTokens ?? null;
  const share = used !== null && windowTokens ? Math.min(100, (used / windowTokens) * 100) : null;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline gap-2">
        <span className="flex-1 font-medium">Context</span>
        <span className="text-muted-foreground tabular-nums">
          {used === null
            ? "not reported yet"
            : `${formatTokens(used)}${windowTokens ? ` of ${formatTokens(windowTokens)}` : ""} tokens${
                share === null ? "" : ` · ${Math.round(share)}%`
              }`}
        </span>
      </div>
      <UsageBar
        role="meter"
        aria-label="Orchestrator context used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={share === null ? undefined : Math.round(share)}
        value={share}
        fill={share !== null && share >= 90 ? "bg-destructive" : "bg-primary"}
        size="md"
      />
      <span className="text-muted-foreground tabular-nums">
        Brigadier injected about {formatTokens(injected)} tokens into this session (≈ 4 bytes
        per token)
      </span>
    </div>
  );
}
