import type { FC } from "react";

import type { DecidedStep } from "@/app/conversation/blocks";
import { WorkerLine } from "@/app/conversation/WorkerChip";
import { ROW, ROW_DETAIL, ThreadActivity } from "@/components/conversation";
import { CheckCircle } from "@/components/icons";
import { cn } from "@/lib/utils";

/** "Decided: …", opening to why. */
export const DecidedRow: FC<{ kind: DecidedStep }> = ({ kind }) => (
  <ThreadActivity
    data-slot="orchestrator-step"
    data-kind="decided"
    className={ROW}
    detail={
      kind.why ? (
        <>
          <span className="text-foreground/80">
            <WorkerLine text={kind.what} />
          </span>
          <span>
            <WorkerLine text={kind.why} />
          </span>
        </>
      ) : undefined
    }
    detailClassName={cn(ROW_DETAIL, "wrap-break-word")}
  >
    <CheckCircle aria-hidden className="size-4 shrink-0" />
    <span className="min-w-0 truncate">
      Decided: <WorkerLine text={kind.what} />
    </span>
  </ThreadActivity>
);
