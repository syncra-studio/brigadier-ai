import { useRef, useState } from "react";

import { useTargetPanel } from "@/app/settings/routing/pickers";
import { effortRank } from "@/app/settings/routing/ruleWords";
import { rowModelName, rowTarget } from "@/app/settings/routing/words";
import { effortLabel, type ModelGroup, ModelPickerPanel, type ModelPickerHandle } from "@/components/shared";
import { SettingsSelectTrigger } from "@/components/controls";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { Check, DotsHorizontal, GripIcon, ProviderGlyph } from "@/components/icons";
import type { useDragReorder } from "@/hooks/use-drag-reorder";
import type { GuidanceRow, ModelRule, OverrideTarget } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** The efforts a row may ask for when its model doesn't list its own. */
const EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;

/** The levels the row's model takes, up to the lowest effort ceiling your rules set for its work. */
function effortLevels(row: GuidanceRow, groups: readonly ModelGroup[], rules: readonly ModelRule[]): readonly string[] {
  const model = groups.find((group) => group.provider === row.agent)?.models.find((found) => found.id === row.model);
  const work = row.work.trim().toLowerCase();
  const ceilings = rules.flatMap((rule) =>
    rule.type === "effortCeiling" && (!rule.work || rule.work.trim().toLowerCase() === work) ? [effortRank(rule.effort)] : [],
  );
  const levels = model && model.efforts.length > 0 ? model.efforts : EFFORTS;
  return ceilings.length > 0 ? levels.filter((level) => effortRank(level) <= Math.min(...ceilings)) : levels;
}

/** One model of your order: grip and place, the model, its effort, and its menu. */
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
}: {
  row: GuidanceRow;
  position: number;
  count: number;
  /** The models of the order, offered disabled when changing this one. */
  taken: readonly OverrideTarget[];
  rules: readonly ModelRule[];
  groups: readonly ModelGroup[];
  dragging: boolean;
  grip: ReturnType<ReturnType<typeof useDragReorder>["grip"]>;
  onEffort: (effort: string | null) => void;
  onModel: (target: OverrideTarget) => void;
  onRemove: () => void;
}) {
  const name = rowModelName(groups, row.agent, row.model);
  const [actions, setActions] = useState(false);
  const picker = useRef<ModelPickerHandle>(null);
  const target = useTargetPanel({
    // A pick closes the whole menu.
    onPick: (picked) => {
      setActions(false);
      onModel(picked);
    },
    taken: [rowTarget(groups, row.agent, row.model), ...taken],
    opensOn: row.agent,
  });
  return (
    <li data-slot="ranked-place" className={cn("flex h-11 items-center gap-3 rounded-lg", dragging && "bg-foreground/8")}>
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
      <span className="flex min-w-0 flex-1 items-center gap-2">
        <ProviderGlyph provider={row.agent} className="shrink-0 size-3.5" />
        <span className="text-label text-foreground truncate">{name}</span>
      </span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <SettingsSelectTrigger label={`${name}: effort`}>{row.effort ? `${effortLabel(row.effort)} effort` : "Default effort"}</SettingsSelectTrigger>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-2xs">
          <DropdownMenuRadioGroup value={row.effort ?? "default"} onValueChange={(value) => onEffort(value === "default" ? null : value)}>
            <DropdownMenuRadioItem value="default" indicator={<Check className="size-icon-md" />}>
              Default
              <span className="text-muted-foreground ms-auto text-xs">the CLI picks</span>
            </DropdownMenuRadioItem>
            {effortLevels(row, groups, rules).map((level) => (
              <DropdownMenuRadioItem key={level} value={level} indicator={<Check className="size-icon-md" />}>
                {effortLabel(level)} effort
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu modal={false} open={actions} onOpenChange={setActions}>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`${name}: actions`}
            className="text-muted-foreground hover:text-foreground flex h-full shrink-0 items-center"
          >
            <DotsHorizontal aria-hidden className="size-3.75" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-2xs">
          <DropdownMenuLabel>{name}</DropdownMenuLabel>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>Change model</DropdownMenuSubTrigger>
            <DropdownMenuSubContent
              data-slot="model-picker-submenu"
              className="p-1"
              // Escape clears the picker's search first, then closes the menu.
              onEscapeKeyDown={(event) => picker.current?.escape() && event.preventDefault()}
            >
              <ModelPickerPanel ref={picker} {...target} onClose={() => setActions(false)} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onRemove}>Remove</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
