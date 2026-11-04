import { ModelEffort } from "@/app/settings/routing/ModelEffort";
import type { ModelGroup } from "@/components/shared";
import type { GuidanceRow } from "@/ipc/generated";

/** "✻ Opus 5.5 · High": the agent, model and effort a kind of work's row names. */
export function NowPick({ row, groups }: { row: GuidanceRow; groups: readonly ModelGroup[] }) {
  return (
    <span data-slot="now-pick" className="flex shrink-0 items-center">
      <ModelEffort row={row} groups={groups} small />
    </span>
  );
}
