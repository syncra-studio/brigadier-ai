import { TargetPicker } from "@/app/settings/routing/pickers";
import { rowModelName, rowTarget } from "@/app/settings/routing/words";
import { SettingsSelectTrigger, TooltipIconButton } from "@/components/controls";
import { GripIcon, ProviderGlyph, X } from "@/components/icons";
import { effortLabel, type ModelGroup } from "@/components/shared";
import type { useDragReorder } from "@/hooks/use-drag-reorder";
import type { GuidanceRow, ModelRule, OverrideTarget } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** One model of your order: grip and place, the picker of its model and effort, and Remove. */
export function OrderPlace({
  row,
  position,
  count,
  taken,
  rules,
  groups,
  dragging,
  grip,
  onEffort,
  onModel,
  onRemove,
  role,
}: {
  row: GuidanceRow;
  position: number;
  count: number;
  /** The models of the order, offered disabled when changing this one. */
  taken: readonly OverrideTarget[];
  rules: readonly ModelRule[];
  groups: readonly ModelGroup[];
  dragging: boolean;
  grip?: ReturnType<ReturnType<typeof useDragReorder>["grip"]>;
  onEffort: (effort: string | null) => void;
  onModel: (target: OverrideTarget, effort: string | null) => void;
  onRemove?: () => void;
  /** A fixed role has no place to reorder or remove, and stays on its agent. */
  role?: string;
}) {
  const name = rowModelName(groups, row.agent, row.model);
  return (
    <li data-slot="ranked-place" className={cn("flex h-11 items-center gap-3 rounded-lg", dragging && "bg-foreground/8")}>
      {role ? (
        <span className="text-xs text-muted-foreground w-44 shrink-0 truncate">{role}</span>
      ) : (
        <button
          type="button"
          aria-label={`Move ${name} (${position} of ${count}); drag, or press up and down`}
          title="Drag to reorder, or focus and press ↑ ↓"
          disabled={count < 2}
          className="flex h-full cursor-grab touch-none items-center gap-3 active:cursor-grabbing disabled:cursor-default"
          {...grip}
        >
          <GripIcon className="text-muted-foreground" />
          <span className="text-xs text-muted-foreground w-2 font-mono">{position}</span>
        </button>
      )}
      <span className="min-w-0 flex-1">
        <TargetPicker
          label={`${name}: model and effort`}
          onPick={onModel}
          taken={[rowTarget(groups, row.agent, row.model), ...taken]}
          opensOn={row.agent}
          chosen={row}
          onEffort={onEffort}
          work={row.work}
          rules={rules}
          {...(role ? { provider: row.agent } : {})}
        >
          <SettingsSelectTrigger
            label={`${name}: model and effort`}
            data-slot="order-place-model"
            icon={<ProviderGlyph provider={row.agent} className="size-3.5 shrink-0" />}
          >
            {name}
            <span className="text-muted-foreground"> · {row.effort ? effortLabel(row.effort) : "Default"}</span>
          </SettingsSelectTrigger>
        </TargetPicker>
      </span>
      {onRemove && (
        <TooltipIconButton tooltip="Remove" aria-label={`Remove ${name}`} onClick={onRemove} className="text-muted-foreground size-icon-button-sm shrink-0">
          <X className="size-icon-sm" />
        </TooltipIconButton>
      )}
    </li>
  );
}
