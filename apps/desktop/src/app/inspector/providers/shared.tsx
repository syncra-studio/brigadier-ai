import { type ReactNode, useState } from "react";

import type { ProviderKind, QuotaSnapshot, RawState } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";
import { usageTone } from "@/lib/quota";
import { cn } from "@/lib/utils";

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
        const usage = usageTone(used, { normal: { text: "text-muted-foreground", fill: "bg-primary" } });
        const resetsAt = window.resetsAtMs;
        return (
          <div key={window.id} className="flex flex-col gap-0.5">
            <div className="flex items-baseline gap-2">
              <span className="flex-1">{window.label}</span>
              <span className={cn("tabular-nums", usage.text)}>
                {Math.round(100 - used)}% left
                {resetsAt !== null && ` · resets ${formatDateTime(resetsAt)}`}
              </span>
            </div>
            <div className="bg-muted h-1 overflow-hidden rounded-full">
              <div
                className={cn("h-full", usage.fill)}
                style={{ width: `${used}%` }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Runs an action, keeping its error for display. */
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

export function Section({
  title,
  actions,
  children,
}: {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2 border-b px-3 py-3">
      <div className="flex items-center gap-2">
        <h3 className="flex-1 text-xs font-medium">{title}</h3>
        {actions}
      </div>
      {children}
    </section>
  );
}
