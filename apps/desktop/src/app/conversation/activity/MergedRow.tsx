import type { FC } from "react";

import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { ROW } from "@/components/conversation";
import { Branch } from "@/components/icons";

/** "Merged feature into main", with where it went when it wasn't pushed. */
export const MergedRow: FC<{ kind: Extract<BlockOrchestratorStep["kind"], { type: "merged" }> }> = ({ kind }) => (
  <div data-slot="orchestrator-step" data-kind="merged" className={ROW}>
    <Branch aria-hidden className="size-4 shrink-0" />
    <span className="min-w-0 truncate">
      Merged {kind.branch} into {kind.base}
      {kind.upstream && ` on this Mac, not pushed to ${kind.upstream.name}`}
      {kind.upstream && kind.upstream.behind > 0 && ` · ${kind.upstream.name} had moved on`}
    </span>
  </div>
);
