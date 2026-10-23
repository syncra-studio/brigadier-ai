import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { rowModelName } from "@/app/settings/routing/words";
import { effortLabel, type ModelGroup } from "@/components/shared";
import { ProviderGlyph } from "@/components/icons";
import type { GuidanceRow } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** Shipped: the shipped row's model and effort, then its reason. */
export function AutoPicks({ row, groups }: { row: GuidanceRow; groups: readonly ModelGroup[] }) {
  return (
    <div data-slot="auto-picks" className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-3")}>
      <div data-slot="auto-pick" className="flex h-10 items-center gap-3">
        <span className="flex min-w-0 flex-1 items-center gap-2">
          <ProviderGlyph provider={row.agent} className="shrink-0 size-3.5" />
          <span className="text-label text-foreground truncate">{rowModelName(groups, row.agent, row.model)}</span>
        </span>
        <span className="text-xs text-muted-foreground shrink-0">{row.effort ? `${effortLabel(row.effort)} effort` : "Default effort"}</span>
      </div>
      <p className="text-xs text-muted-foreground pt-1">{row.reason}</p>
    </div>
  );
}
