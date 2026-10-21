import { OrderPlace } from "@/app/settings/routing/OrderPlace";
import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { TargetItems } from "@/app/settings/routing/pickers";
import type { ModelGroup } from "@/components/shared";
import { Button } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from "@/components/overlays";
import { Plus } from "@/components/icons";
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type { RankedEntry, RoutePreview } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A model of an order's key: its place moves while it is dragged, its target doesn't. */
export function placeKey(entry: RankedEntry): string {
  const target = entry.target;
  return `${target.type}:${target.provider}:${target.type === "family" ? target.family : target.type === "model" ? target.id : ""}`;
}

/** My order: each model dragged by its grip, its effort and menu; then Add model. */
export function OrderList({
  name,
  entries,
  route,
  groups,
  onSave,
}: {
  name: string;
  entries: RankedEntry[];
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  onSave: (entries: RankedEntry[]) => void;
}) {
  const { listRef, shown, dragging, grip } = useDragReorder<RankedEntry, HTMLOListElement>({
    items: entries,
    idOf: placeKey,
    rowSelector: "[data-slot=ranked-place]",
    onMove: (id, to) => {
      const moved = entries.find((entry) => placeKey(entry) === id);
      if (!moved) return;
      const rest = entries.filter((entry) => placeKey(entry) !== id);
      rest.splice(to, 0, moved);
      onSave(rest);
    },
  });
  const taken = entries.map((entry) => entry.target);
  return (
    <div className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-2")}>
      <ol ref={listRef} aria-label={`The order ${name} tries`} className="flex flex-col">
        {shown.map((entry, index) => {
          const key = placeKey(entry);
          const replace = (next: RankedEntry | null) =>
            onSave(entries.flatMap((other) => (placeKey(other) === key ? (next ? [next] : []) : [other])));
          return (
            <OrderPlace
              key={key}
              entry={entry}
              position={index + 1}
              count={entries.length}
              taken={taken}
              route={route}
              groups={groups}
              dragging={dragging === key}
              grip={grip(key, index)}
              onEffort={(effort) => replace({ ...entry, effort })}
              onModel={(target) => replace({ target, effort: null })}
              onRemove={() => replace(null)}
            />
          );
        })}
      </ol>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button variant="link-blue" size="inline" className="text-xs h-9 w-fit gap-2 ps-6.5 font-medium">
            <Plus aria-hidden className="size-3.5" />
            Add model
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
          <TargetItems
            onPick={(target) => onSave([...entries, { target, effort: null }])}
            taken={taken}
            candidates={route?.candidates ?? []}
          />
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
