import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { AutoPicks } from "@/app/settings/routing/AutoPicks";
import { RolePicks } from "@/app/settings/routing/RolePicks";
import { ROLE_KINDS } from "@/app/settings/routing/roles";
import { OrderList } from "@/app/settings/routing/OrderList";
import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import { withOrder, type WorkRow, workName } from "@/app/settings/routing/words";
import { type ModelGroup, RowText } from "@/components/shared";
import { Segmented } from "@/components/controls";
import type { GuidanceRow, ModelRules } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { saveModelRules } from "@/state/modelRules";

/**
 * An opened kind: Who picks (Automatic, the shipped rows; or My order, your rows), then its
 * models. My order starts from the shipped rows; removing its last model goes back to Automatic.
 */
export function WhoPicks({ row, rules, groups }: { row: WorkRow; rules: ModelRules; groups: readonly ModelGroup[] }) {
  const action = useAction();
  // My order with nothing to start it from (a kind of your own): an empty order, Add model there to fill it.
  const [starting, setStarting] = useState(false);
  const name = workName(row.work);
  const role = ROLE_KINDS[row.work];
  const save = (order: GuidanceRow[]) => {
    setStarting(false);
    // Fusion is one display row but keeps its two work names in the rules file.
    const works = [...new Set([...row.shipped, ...row.mine].map((found) => found.work))];
    const rows = role
      ? works.reduce((saved, work) => withOrder(saved, work, order.filter((found) => found.work === work)), rules.rows)
      : withOrder(rules.rows, row.work, order);
    return action.run(() => saveModelRules(rows, rules.rules));
  };
  const choose = (mode: "automatic" | "order") => {
    if (mode === "automatic") {
      setStarting(false);
      // Automatic drops your order (a kind of your own goes with it).
      return row.mine.length > 0 ? save([]) : undefined;
    }
    if (row.shipped.length > 0) return save((role ? row.rows : row.shipped).map((shipped) => ({ ...shipped })));
    setStarting(true);
  };
  const ordering = row.mine.length > 0 || starting;
  return (
    <div data-slot="who-picks" className="flex flex-col ps-7.5 pb-1">
      <div className={cn(HAIRLINE_TOP, "flex items-center gap-6 py-3.5")}>
        <RowText
          truncate
          quiet
          title="Who picks"
          line={
            ordering
              ? role ? "Your models for each role" : "Your order: the first model your rules allow takes it"
              : "Automatic advice, updated with each release"
          }
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
      {role ? (
        <RolePicks row={row} kind={role} groups={groups} rules={rules.rules} onSave={ordering ? save : undefined} />
      ) : ordering ? (
        <OrderList work={row.work} order={row.mine} rules={rules.rules} groups={groups} onSave={save} />
      ) : (
        <AutoPicks row={row} rules={rules.rules} groups={groups} />
      )}
      {action.error && (
        <p role="alert" className="text-destructive text-xs pb-3">
          {action.error}
        </p>
      )}
    </div>
  );
}
