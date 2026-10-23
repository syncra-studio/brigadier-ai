import { OrderPlace } from "@/app/settings/routing/OrderPlace";
import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { TargetItems } from "@/app/settings/routing/pickers";
import { rowTarget, targetModel } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared";
import { Button } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from "@/components/overlays";
import { Plus } from "@/components/icons";
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type { GuidanceRow, ModelRule, OverrideTarget } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** The reason a row of yours gives once you pick its model here. */
const YOUR_CHOICE = "Your choice.";

/** A row of an order's key: its place moves while it is dragged, its model doesn't. */
const placeKey = (row: GuidanceRow) => `${row.agent}:${row.model}`;

/** My order: each model dragged by its grip, its effort and menu; then Add model. */
export function OrderList({
  work,
  order,
  rules,
  groups,
  onSave,
}: {
  work: string;
  order: readonly GuidanceRow[];
  rules: readonly ModelRule[];
  groups: readonly ModelGroup[];
  onSave: (order: GuidanceRow[]) => void;
}) {
  const { listRef, shown, dragging, grip } = useDragReorder<GuidanceRow, HTMLOListElement>({
    items: order,
    idOf: placeKey,
    rowSelector: "[data-slot=ranked-place]",
    onMove: (id, to) => {
      const moved = order.find((row) => placeKey(row) === id);
      if (!moved) return;
      const rest = order.filter((row) => placeKey(row) !== id);
      rest.splice(to, 0, moved);
      onSave(rest);
    },
  });
  const taken = order.map((row) => rowTarget(groups, row.agent, row.model));
  const picked = (target: OverrideTarget): GuidanceRow => ({
    work,
    agent: target.provider,
    model: targetModel(target),
    effort: null,
    reason: YOUR_CHOICE,
  });
  return (
    <div className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-2")}>
      <ol ref={listRef} aria-label={`The order ${work} tries`} className="flex flex-col">
        {shown.map((row, index) => {
          const key = placeKey(row);
          const replace = (next: GuidanceRow | null) =>
            onSave(order.flatMap((other) => (placeKey(other) === key ? (next ? [next] : []) : [other])));
          return (
            <OrderPlace
              key={key}
              row={row}
              position={index + 1}
              count={order.length}
              taken={taken}
              rules={rules}
              groups={groups}
              dragging={dragging === key}
              grip={grip(key, index)}
              onEffort={(effort) => replace({ ...row, effort })}
              onModel={(target) => replace(picked(target))}
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
          <TargetItems onPick={(target) => onSave([...order, picked(target)])} taken={taken} />
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
