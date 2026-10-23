import { TargetItems } from "@/app/settings/routing/pickers";
import { effortRank } from "@/app/settings/routing/ruleWords";
import { rowModelName, rowTarget } from "@/app/settings/routing/words";
import { effortLabel, type ModelGroup } from "@/components/shared";
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
import { Check, DotsHorizontal, ProviderGlyph } from "@/components/icons";
import type { GuidanceRow, ModelRule, OverrideTarget } from "@/ipc/generated";

/** The efforts a row may ask for when its model doesn't list its own. */
const EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;

/** The levels the row's model takes, up to your effort ceiling if you set one. */
function effortLevels(row: GuidanceRow, groups: readonly ModelGroup[], rules: readonly ModelRule[]): readonly string[] {
  const model = groups.find((group) => group.provider === row.agent)?.models.find((found) => found.id === row.model);
  const ceiling = rules.find((rule) => rule.type === "effortCeiling");
  const levels = model && model.efforts.length > 0 ? model.efforts : EFFORTS;
  return ceiling?.type === "effortCeiling" ? levels.filter((level) => effortRank(level) <= effortRank(ceiling.effort)) : levels;
}

/** Your row of a kind of work: its model, its effort, and its menu. */
export function OrderPlace({
  row,
  rules,
  groups,
  onEffort,
  onModel,
  onRemove,
}: {
  row: GuidanceRow;
  rules: readonly ModelRule[];
  groups: readonly ModelGroup[];
  onEffort: (effort: string | null) => void;
  onModel: (target: OverrideTarget) => void;
  onRemove: () => void;
}) {
  const name = rowModelName(groups, row.agent, row.model);
  const taken: OverrideTarget[] = [rowTarget(groups, row.agent, row.model)];
  return (
    <li data-slot="ranked-place" className="flex h-11 items-center gap-3 rounded-lg">
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
                {effortLabel(level)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu modal={false}>
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
            <DropdownMenuSubContent className="max-h-96 w-xs overflow-y-auto">
              <TargetItems onPick={onModel} taken={taken} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onRemove}>Remove</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
