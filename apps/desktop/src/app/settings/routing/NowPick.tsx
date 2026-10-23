import { rowModelName } from "@/app/settings/routing/words";
import { effortLabel, type ModelGroup } from "@/components/shared";
import { ProviderGlyph } from "@/components/icons";
import type { GuidanceRow } from "@/ipc/generated";

/** "✻ Opus 5.5 · High": the agent, model and effort a kind of work's row names. */
export function NowPick({ row, groups }: { row: GuidanceRow; groups: readonly ModelGroup[] }) {
  return (
    <span data-slot="now-pick" className="flex shrink-0 items-center gap-1.75">
      <ProviderGlyph provider={row.agent} className="shrink-0 size-3.5" />
      <span className="text-xs text-foreground">
        {rowModelName(groups, row.agent, row.model)}
        {row.effort && <span className="text-muted-foreground"> · {effortLabel(row.effort)}</span>}
      </span>
    </span>
  );
}
