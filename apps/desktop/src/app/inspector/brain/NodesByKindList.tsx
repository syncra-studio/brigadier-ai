import { KINDS, NODE_KINDS } from "@/app/inspector/brain/kinds";
import { count } from "@/app/inspector/brain/overviewText";
import type { BrainOverview } from "@/ipc/generated";

/** How many nodes the Brain holds of each kind, every kind listed. */
export function NodesByKindList({ stats }: { stats: BrainOverview["stats"] }) {
  return (
    <ul aria-label="Nodes by kind" className="grid grid-cols-2 gap-x-4 gap-y-1">
      {NODE_KINDS.map((kind) => {
        const nodes = stats.kinds.find((entry) => entry.kind === kind)?.nodes ?? 0;
        return (
          <li key={kind} className="flex items-center gap-1.5">
            <span aria-hidden className={`${KINDS[kind].swatch} size-2 shrink-0 rounded-full`} />
            <span className={nodes === 0 ? "text-muted-foreground flex-1" : "flex-1"}>
              {KINDS[kind].label}
            </span>
            <span className="text-muted-foreground tabular-nums">{count.format(nodes)}</span>
          </li>
        );
      })}
    </ul>
  );
}
