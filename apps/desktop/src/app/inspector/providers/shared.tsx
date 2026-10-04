import type { ProviderKind, QuotaSnapshot, RawState } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";

export const PROVIDER_LABELS: Record<ProviderKind, string> = {
  claude: "Claude Code",
  codex: "Codex",
};

export const STATE_VARIANTS: Record<RawState, "secondary" | "success" | "warning" | "destructive" | "outline"> = {
  starting: "warning",
  running: "success",
  stopped: "secondary",
  failed: "destructive",
  closing: "warning",
  closed: "outline",
};

/** Usage windows as bars of what is used, with what is left and when each resets. */
export function QuotaWindows({ quota }: { quota: QuotaSnapshot }) {
  return (
    <div className="flex flex-col gap-1.5">
      {quota.limit && (
        <p className="text-destructive">
          Limit reached{quota.limit.window ? ` (${quota.limit.window})` : ""}
          {quota.limit.resetsAtMs !== null &&
            ` · resets ${formatDateTime(quota.limit.resetsAtMs)}`}
        </p>
      )}
      {quota.windows.map((window) => {
        const used = Math.min(100, Math.max(0, window.usedPercent));
        const resetsAt = window.resetsAtMs;
        return (
          <div key={window.id} className="flex flex-col gap-0.5">
            <div className="flex items-baseline gap-2">
              <span className="flex-1">{window.label}</span>
              <span className="text-muted-foreground tabular-nums">
                {Math.round(100 - used)}% left
                {resetsAt !== null && ` · resets ${formatDateTime(resetsAt)}`}
              </span>
            </div>
            <div className="bg-muted h-1 overflow-hidden rounded-full">
              <div
                className={used >= 90 ? "bg-destructive h-full" : "bg-primary h-full"}
                style={{ width: `${used}%` }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}
