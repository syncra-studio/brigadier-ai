import { rowModelName } from "@/app/settings/routing/words";
import { effortLabel, type ModelGroup } from "@/components/shared";
import { ProviderGlyph } from "@/components/icons";
import type { GuidanceRow } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A dot between two words, centred on their line. */
export function MidDot() {
  return <span aria-hidden className="bg-muted-foreground size-0.75 shrink-0 rounded-full" />;
}

/** "✻ Opus 5.5 · High": the agent, the model, then its effort after a dot (no effort: just the model). */
export function ModelEffort({ row, groups, small = false }: { row: GuidanceRow; groups: readonly ModelGroup[]; small?: boolean }) {
  const text = small ? "text-xs" : "text-label";
  return (
    <span className={cn("flex min-w-0 items-center", small ? "gap-1.75" : "gap-2")}>
      <ProviderGlyph provider={row.agent} className="shrink-0 size-3.5" />
      <span className="flex min-w-0 items-center gap-1.5">
        <span className={cn(text, "text-foreground truncate")}>{rowModelName(groups, row.agent, row.model)}</span>
        {row.effort && (
          <>
            <MidDot />
            <span className={cn(text, "text-muted-foreground shrink-0")}>{effortLabel(row.effort)}</span>
          </>
        )}
      </span>
    </span>
  );
}
