import { ModelEffort } from "@/app/settings/routing/ModelEffort";
import { OrderPlace } from "@/app/settings/routing/OrderPlace";
import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import type { RoleKind } from "@/app/settings/routing/roles";
import { targetModel, type WorkRow } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared";
import type { GuidanceRow, ModelRule } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A kind of roles: each role and the model it runs on, with its effort; then what the kind does. */
export function RolePicks({ row, kind, groups, rules, onSave }: {
  row: WorkRow;
  kind: RoleKind;
  groups: readonly ModelGroup[];
  rules: readonly ModelRule[];
  onSave?: ((rows: GuidanceRow[]) => void) | undefined;
}) {
  const replace = (found: GuidanceRow, next: GuidanceRow) =>
    onSave?.(row.rows.map((other) => other === found ? next : other));
  return (
    <div data-slot="role-picks" className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-3")}>
      <ul>
        {row.rows.map((found) => onSave ? (
          <OrderPlace
            key={`${found.work}/${found.agent}/${found.model}`}
            row={found}
            role={kind.role(found)}
            groups={groups}
            rules={rules}
            position={1}
            count={1}
            taken={[]}
            dragging={false}
            onEffort={(effort) => replace(found, { ...found, effort })}
            onModel={(target) => replace(found, { ...found, model: targetModel(target), effort: null, reason: "Your choice." })}
          />
        ) : (
          <li key={`${found.work}/${found.agent}/${found.model}`} data-slot="role-pick" className="flex h-10 items-center gap-3">
            <span className="text-xs text-muted-foreground w-44 shrink-0 truncate">{kind.role(found)}</span>
            <ModelEffort row={found} groups={groups} />
          </li>
        ))}
      </ul>
      <p className="text-xs text-muted-foreground pt-1">{kind.note}</p>
    </div>
  );
}
