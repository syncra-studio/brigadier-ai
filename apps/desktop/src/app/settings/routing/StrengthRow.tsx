import { HAIRLINE_BOTTOM } from "@/app/settings/routing/parts";
import { faded, tierFor } from "@/app/settings/routing/words";
import { ProviderGlyph } from "@/components/icons";
import type { MergedModel, TaskCategory } from "@/ipc/generated";
import { TIER_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";

/**
 * A model ranked in Model strength: its place, name and tier, its strength as a bar and a
 * number.
 */
export function StrengthRow({
  model,
  strength,
  place,
  category,
}: {
  model: MergedModel;
  strength: number;
  place: number;
  category: TaskCategory;
}) {
  const dim = faded(model);
  return (
    <div
      data-slot="strength-row"
      className={cn(HAIRLINE_BOTTOM, "flex items-center gap-3.5 py-3")}
    >
      <span className="text-xs text-muted-foreground w-3.5 shrink-0 font-mono">{place}</span>
      <span className={cn("flex min-w-0 flex-1 items-center gap-2.5", dim && "opacity-50")}>
        <ProviderGlyph provider={model.provider} className="shrink-0 size-3.75" />
        <span className="flex min-w-0 items-center gap-2">
          <span data-slot="strength-name" className="text-label text-foreground truncate font-medium">
            {model.displayName}
          </span>
          <span className="text-xs text-muted-foreground shrink-0">
            {TIER_LABELS[tierFor(model, category)]}
          </span>
        </span>
      </span>
      <span aria-hidden className={cn("bg-muted h-1 w-105 shrink rounded-full", dim && "opacity-50")}>
        <span
          className={"bg-muted-foreground/60 block h-full rounded-full"}
          style={{ width: `${Math.max(0, Math.min(10, strength)) * 10}%` }}
        />
      </span>
      <span
        className={cn(
          "text-xs w-7.5 shrink-0 text-end font-mono font-medium",
          dim ? "text-muted-foreground" : "text-foreground",
        )}
      >
        {strength.toFixed(1)}
      </span>
      <span className="flex min-w-23 shrink-0 justify-end" />
    </div>
  );
}
