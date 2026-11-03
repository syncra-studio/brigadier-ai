import { ModelEffort } from "@/app/settings/routing/ModelEffort";
import { HAIRLINE_TOP } from "@/app/settings/routing/parts";
import type { RoleKind } from "@/app/settings/routing/roles";
import type { WorkRow } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared";
import { cn } from "@/lib/utils";

/** Automatic for a kind of roles: each role and the model it runs on, with its effort; then what the kind does. */
export function RolePicks({ row, kind, groups }: { row: WorkRow; kind: RoleKind; groups: readonly ModelGroup[] }) {
  return (
    <div data-slot="role-picks" className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-3")}>
      {row.shipped.map((found) => (
        <div key={`${found.work}/${found.agent}/${found.model}`} data-slot="role-pick" className="flex h-10 items-center gap-3">
          <span className="text-xs text-muted-foreground w-44 shrink-0 truncate">{kind.role(found)}</span>
          <ModelEffort row={found} groups={groups} />
        </div>
      ))}
      <p className="text-xs text-muted-foreground pt-1">{kind.note}</p>
    </div>
  );
}
