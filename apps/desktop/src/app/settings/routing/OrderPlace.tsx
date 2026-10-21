import { TargetItems } from "@/app/settings/routing/pickers";
import { shortPlaceName } from "@/app/settings/routing/words";
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
import { Check, DotsHorizontal, GripIcon, ProviderGlyph } from "@/components/icons";
import type { useDragReorder } from "@/hooks/use-drag-reorder";
import type { OverrideTarget, RankedEntry, RoutePreview } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** The efforts a model of an order may ask for: routing never goes above high. */
const EFFORTS = ["low", "medium", "high"] as const;

/** Auto, then the levels up to high the model takes (all three for a family or a vendor). */
function effortLevels(target: OverrideTarget, groups: readonly ModelGroup[]): readonly string[] {
  const model =
    target.type === "model"
      ? groups.find((group) => group.provider === target.provider)?.models.find((found) => found.id === target.id)
      : undefined;
  return model ? EFFORTS.filter((level) => model.efforts.includes(level)) : EFFORTS;
}

/** One model of an order: grip and place, the model, its effort, and its menu. */
export function OrderPlace({
  entry,
  position,
  count,
  taken,
  route,
  groups,
  dragging,
  grip,
  onEffort,
  onModel,
  onRemove,
}: {
  entry: RankedEntry;
  position: number;
  count: number;
  taken: readonly OverrideTarget[];
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  dragging: boolean;
  grip: ReturnType<ReturnType<typeof useDragReorder>["grip"]>;
  onEffort: (effort: string | null) => void;
  onModel: (target: OverrideTarget) => void;
  onRemove: () => void;
}) {
  const target = entry.target;
  const name = shortPlaceName(target, groups);
  // The effort it runs at: its own, else the one routing would give it now.
  const place = route?.places.find((found) => found.position === position);
  const resolved = place?.model ?? (target.type === "model" ? target.id : null);
  const candidate = route?.candidates.find((found) => found.provider === target.provider && found.model === resolved);
  const effort = entry.effort ?? candidate?.effort ?? null;
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
        <ProviderGlyph provider={target.provider} className="shrink-0 size-3.5" />
        <span className="text-label text-foreground truncate">{name}</span>
      </span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <SettingsSelectTrigger label={`${name}: effort`}>{effort ? `${effortLabel(effort)} effort` : "Auto effort"}</SettingsSelectTrigger>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-2xs">
          <DropdownMenuRadioGroup value={entry.effort ?? "auto"} onValueChange={(value) => onEffort(value === "auto" ? null : value)}>
            <DropdownMenuRadioItem value="auto" indicator={<Check className="size-icon-md" />}>
              Auto
              <span className="text-muted-foreground ms-auto text-xs">routing picks</span>
            </DropdownMenuRadioItem>
            {effortLevels(target, groups).map((level) => (
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
              <TargetItems onPick={onModel} taken={taken} candidates={route?.candidates ?? []} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onRemove}>Remove</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
