import { useAction } from "@/app/conversation/useAction";
import { AutoPicks } from "@/app/settings/routing/AutoPicks";
import { OrderPlace } from "@/app/settings/routing/OrderPlace";
import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { targetModel, type WorkRow } from "@/app/settings/routing/words";
import { type ModelGroup, RowText } from "@/components/shared";
import { Segmented } from "@/components/controls";
import type { GuidanceRow, ModelRules } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { saveModelRules } from "@/state/modelRules";

/** The reason a row of yours gives once you pick its model here. */
const YOUR_CHOICE = "Your choice.";

/** An opened kind: Who picks (the shipped row, or yours), then its model. */
export function WhoPicks({ row, rules, groups }: { row: WorkRow; rules: ModelRules; groups: readonly ModelGroup[] }) {
  const action = useAction();
  const name = row.work;
  // Your row replaces the one of its work, in its place; a new one goes last.
  const save = (next: GuidanceRow | null) =>
    action.run(() => {
      const rest = rules.rows.filter((found) => found.work !== row.work);
      const at = rules.rows.findIndex((found) => found.work === row.work);
      const rows = next === null ? rest : at < 0 ? [...rest, next] : [...rest.slice(0, at), next, ...rest.slice(at)];
      return saveModelRules(rows, rules.rules);
    });
  // Shipped drops your row (a kind of your own goes with it); Mine starts yours from the shipped one.
  const choose = (mode: "shipped" | "mine") => {
    if (mode === "shipped") return save(null);
    if (!row.mine && row.shipped) return save({ ...row.shipped });
  };
  const mine = row.mine;
  return (
    <div data-slot="who-picks" className="flex flex-col ps-7.5 pb-1">
      <div className={cn(HAIRLINE_TOP, "flex items-center gap-6 py-3.5")}>
        <RowText
          truncate
          quiet
          title="Who picks"
          line={mine ? "Your row, in ~/.brigadier/models.md" : "The shipped row, updated with each release"}
        />
        <Segmented
          label={`Whose row ${name} follows`}
          value={mine ? "mine" : "shipped"}
          options={[
            { value: "shipped", label: "Shipped" },
            { value: "mine", label: "Mine" },
          ]}
          disabled={action.busy}
          onChange={choose}
        />
      </div>
      {mine ? (
        <ul aria-label={`What ${name} runs on`} className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-2")}>
          <OrderPlace
            row={mine}
            rules={rules.rules}
            groups={groups}
            onEffort={(effort) => save({ ...mine, effort })}
            onModel={(target) => save({ ...mine, agent: target.provider, model: targetModel(target), effort: null, reason: YOUR_CHOICE })}
            onRemove={() => save(null)}
          />
        </ul>
      ) : (
        row.shipped && <AutoPicks row={row.shipped} groups={groups} />
      )}
      {action.error && (
        <p role="alert" className="text-destructive text-xs pb-3">
          {action.error}
        </p>
      )}
    </div>
  );
}
