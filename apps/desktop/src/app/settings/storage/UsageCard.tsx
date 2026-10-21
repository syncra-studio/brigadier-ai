import { size, usageParts } from "@/app/settings/storage/model";
import { UsageBar } from "@/app/settings/storage/parts";
import { FolderOpen } from "@/components/icons";
import { SettingsCard, StateDot } from "@/components/shared";
import { revealPath } from "@/ipc/client";
import type { StorageReport } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";

/** What Brigadier uses in all, where, and what it is made of. */
export function UsageCard({ report, error }: { report: StorageReport | null; error: string | null }) {
  const parts = report ? usageParts(report) : [];
  return (
    <SettingsCard padded data-usage>
      <div className="flex items-end gap-4">
        <div className="flex min-w-0 flex-1 items-end gap-2.5">
          <span className="text-page-title font-medium">
            {report ? size(report.totalBytes) : "…"}
          </span>
          <span className="text-foreground/65 text-label leading-none">
            {report ? "used by Brigadier" : "Looking…"}
          </span>
        </div>
        {report?.dataDir && (
          <button
            type="button"
            title={report.dataDir}
            onClick={() => void revealPath(report.dataDir).catch(() => {})}
            className="text-muted-foreground hover:text-foreground/65 flex min-w-0 items-center gap-2 font-mono text-xs"
          >
            <span className="truncate">{shortPath(report.dataDir)}</span>
            <FolderOpen aria-hidden className="text-foreground/65 size-3.5 shrink-0" />
          </button>
        )}
      </div>
      {error && <span className="text-destructive text-xs">{error}</span>}
      <UsageBar parts={parts} pending={!report} />
      <div className="flex justify-between gap-4">
        {parts.map((part) => (
          <div key={part.key} data-legend={part.key} className="flex flex-col gap-1.25">
            <span className="text-foreground/65 flex items-center gap-1.75 text-xs">
              <StateDot color={part.color} />
              {part.label}
            </span>
            <span
              className={cn(
                "text-xs font-medium",
                part.bytes > 0 ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {size(part.bytes)}
            </span>
          </div>
        ))}
      </div>
    </SettingsCard>
  );
}
