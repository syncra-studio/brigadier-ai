import { useEffect, useState } from "react";

import { ROW } from "@/app/conversation/railMenu";
import { resetsAt, windowName } from "@/app/conversation/StatusCard";
import { ChevronDown, ChevronRight, SpeedometerLatencySpeed } from "@/components/icons";
import type { QuotaWindow } from "@/ipc/generated";
import { usageTone } from "@/lib/quota";
import { cn } from "@/lib/utils";
import { loadProviders } from "@/state/actions";
import { useApp } from "@/state/store";

/** A usage window's name: "Weekly", "5h". */
function usageName(window: QuotaWindow): string {
  return window.windowMinutes === 10080 ? "Weekly" : windowName(window);
}

/** "Usage remaining ›": the model's provider's windows, expanded in place. */
export function UsageRemaining({ provider }: { provider: string }) {
  const [open, setOpen] = useState(false);
  const view = useApp((s) => s.providers.view);
  useEffect(() => {
    if (open && !view) void loadProviders().catch(() => {});
  }, [open, view]);
  const windows =
    view?.providers.find((overview) => overview.provider === provider)?.quota?.windows ?? [];
  return (
    <>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className={cn(ROW, "hover:bg-foreground/8")}
      >
        <SpeedometerLatencySpeed />
        <span className="flex-1">Usage remaining</span>
        {open ? <ChevronDown /> : <ChevronRight />}
      </button>
      {open && (
        <div className="flex flex-col gap-1 px-2 pb-1 ps-8 text-sm">
          {windows.length === 0 && (
            <p className="text-muted-foreground">{view ? "Not reported yet" : "Loading…"}</p>
          )}
          {windows.map((window) => (
            <p key={window.id} className="flex items-center gap-2">
              <span className="flex-1">{usageName(window)}</span>
              <span className={cn("tabular-nums", usageTone(window.usedPercent, { normal: { text: "" } }).text)}>
                {Math.max(0, Math.round(100 - window.usedPercent))}%
              </span>
              {window.resetsAtMs !== null && (
                <span className="text-muted-foreground">{resetsAt(window.resetsAtMs, window)}</span>
              )}
            </p>
          ))}
        </div>
      )}
    </>
  );
}
