import type { FC } from "react";

import { Capsule } from "@/app/conversation/Capsule";
import { CapsulePill } from "@/app/conversation/CapsulePill";
import { DiffPill } from "@/app/conversation/DiffPill";
import { useRunDiff } from "@/app/conversation/overnightAdapter";
import type { RunPill } from "@/app/conversation/phaseView";
import { StepList } from "@/app/conversation/StepList";
import { Donut } from "@/components/conversation";

/** The capsule of an overnight run: its phase, the phase's steps and the run branch's diff. */
export const RunCapsule: FC<{ conversationId: string; run: RunPill }> = ({ conversationId, run }) => {
  const runDiff = useRunDiff(conversationId, run.runId);
  // Nothing landed yet: no "0 files changed".
  const diff = runDiff && runDiff.files.length > 0 ? runDiff : null;
  return (
    <Capsule>
      <CapsulePill
        tip={
          run.steps.length > 0 ? (
            <StepList
              steps={run.steps.map((step) => ({
                title: step.title,
                status: step.done ? "done" : step.active ? "active" : "pending",
              }))}
            />
          ) : (
            run.label
          )
        }
      >
        {run.total > 0 && <Donut done={run.done} total={run.total} />}
        <span className="text-foreground tabular-nums">{run.label}</span>
      </CapsulePill>
      {diff && <span aria-hidden>·</span>}
      {diff && <DiffPill diff={diff} />}
    </Capsule>
  );
};
