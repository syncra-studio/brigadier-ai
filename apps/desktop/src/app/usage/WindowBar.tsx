import type { QuotaWarning, QuotaWindow } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { tone, used } from "@/lib/quota";
import { cn } from "@/lib/utils";

/** Weekly quota about to be lost unused, in one line: who, how much is left, when it resets. */
export function QuotaWarningLine({ warning, now }: { warning: QuotaWarning; now: number }) {
  return (
    <p data-slot="quota-warning" className="text-warning px-2 py-1.5 text-xs">
      {warning.who} has {Math.round(warning.leftPercent)}% of its weekly quota left; it resets in{" "}
      {formatCountdown(warning.resetsAtMs, now)}
    </p>
  );
}

/** One usage window: its name, how much is used, when it resets, and a bar. */
export function WindowBar({ window, now }: { window: QuotaWindow; now: number }) {
  const percent = used(window);
  return (
    <div className="flex flex-col gap-1 text-xs">
      <div className="flex items-center gap-2 tabular-nums">
        <span className="text-muted-foreground min-w-0 flex-1 truncate">{window.label}</span>
        <span className={tone(percent).text}>{percent}% used</span>
        {window.resetsAtMs !== null && (
          <span className="text-muted-foreground">{formatCountdown(window.resetsAtMs, now)}</span>
        )}
      </div>
      <span aria-hidden className="bg-muted rounded-capsule h-1 overflow-hidden">
        <span className={cn("block h-full", tone(percent).fill)} style={{ width: `${percent}%` }} />
      </span>
    </div>
  );
}
