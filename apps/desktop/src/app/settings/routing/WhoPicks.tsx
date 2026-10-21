import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { AutoPicks } from "@/app/settings/routing/AutoPicks";
import { OrderList } from "@/app/settings/routing/OrderList";
import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { type ModelGroup, RowText } from "@/components/shared";
import { Segmented } from "@/components/controls";
import type { RankedEntry, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { PLAIN_KIND_NAMES } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { changeRanking, newRankingId } from "@/state/routing";

/** An opened kind: Who picks (Automatic or My order), then its models. */
export function WhoPicks({
  category,
  own,
  deciding,
  route,
  groups,
  onSeeStrengths,
}: {
  category: TaskCategory;
  own: Ranking | null;
  deciding: Ranking | null;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  onSeeStrengths: () => void;
}) {
  const action = useAction();
  const name = PLAIN_KIND_NAMES[category];
  // My order with nothing to start it from: an empty order, its Add model there to fill it.
  const [starting, setStarting] = useState(false);
  const save = (next: RankedEntry[]) => {
    setStarting(false);
    return action.run(() =>
      changeRanking(
        own?.id ?? "",
        // Removing the last model leaves the kind on Automatic.
        (current) => ({ ...current, entries: next, manual: next.length > 0 }),
        () => created(true, []),
      ),
    );
  };
  const created = (manual: boolean, entries: RankedEntry[]): Ranking => ({
    id: newRankingId(),
    category,
    areas: [],
    projectId: null,
    manual,
    entries,
    only: false,
    updatedAtMs: Date.now(),
  });
  // Automatic keeps the order to come back to; My order brings it back, else starts one with
  // the model picked now.
  const choose = (mode: "automatic" | "order") => {
    if (mode === "automatic") {
      setStarting(false);
      return action.run(() => changeRanking(own?.id ?? "", (current) => ({ ...current, manual: false }), () => created(false, [])));
    }
    if (own && own.entries.length > 0) return action.run(() => changeRanking(own.id, (current) => ({ ...current, manual: true })));
    const picked = route?.outcome.type === "chosen" ? route.outcome.choice : null;
    if (!picked) return setStarting(true);
    return save([{ target: { type: "model", provider: picked.provider, id: picked.model ?? "" }, effort: null }]);
  };
  const ordering = deciding !== null || starting;
  return (
    <div data-slot="who-picks" className="flex flex-col ps-7.5 pb-1">
      <div className={cn(HAIRLINE_TOP, "flex items-center gap-6 py-3.5")}>
        <RowText
          truncate
          quiet
          title="Who picks"
          line={ordering ? "Brigadier, or the order you set" : "The strongest model that can run right now"}
        />
        <Segmented
          label={`How ${name} picks a model`}
          value={ordering ? "order" : "automatic"}
          options={[
            { value: "automatic", label: "Automatic" },
            { value: "order", label: "My order" },
          ]}
          disabled={action.busy}
          onChange={choose}
        />
      </div>
      {ordering ? (
        <OrderList name={name} entries={deciding?.entries ?? []} route={route} groups={groups} onSave={save} />
      ) : (
        <AutoPicks category={category} route={route} groups={groups} onSeeStrengths={onSeeStrengths} />
      )}
      {action.error && (
        <p role="alert" className="text-destructive text-xs pb-3">
          {action.error}
        </p>
      )}
    </div>
  );
}
