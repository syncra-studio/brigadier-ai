import { Branch, CheckCircle, Clock, PauseCircle, PlayCircle } from "@openai/apps-sdk-ui/components/Icon";
import type { FC } from "react";

import { isTeamStep, TeamStep, WorkerName } from "@/app/conversation/activity/TeamSentence";
import type { BlockOrchestratorStep, DecidedStep, MachineWords } from "@/app/conversation/blocks";
import { machineWords } from "@/app/conversation/rowWords";
import { SandboxLimitNotice } from "@/app/conversation/SandboxLimitNotice";
import { WorkerLine } from "@/app/conversation/WorkerChip";
import { ROW, ROW_DETAIL } from "@/components/assistant-ui/elements/activity-row";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

/**
 * The thread's notices among its work (THREAD-UX-PLAN.md §3.1): a judgement call the lead made,
 * the machine pausing the work, a merge the user asked for, the session's sandbox stopping what
 * the user asked for. Each is one standalone row; the last has its own buttons.
 */

const MACHINE_ICONS: Record<MachineWords["machine"], FC<{ className?: string }>> = {
  waitingToCool: Clock,
  waitingForBuild: Clock,
  paused: PauseCircle,
  resumed: PlayCircle,
};

/** A row about the machine; it names the worker when it is about one. */
const MachineRow: FC<{ kind: MachineWords }> = ({ kind }) => {
  const mac = useApp((s) => s.info?.platform === "macos");
  const Icon = MACHINE_ICONS[kind.machine];
  return (
    <div data-slot="orchestrator-step" data-kind="machine" className={ROW}>
      <Icon aria-hidden className="size-4 shrink-0" />
      <span className="flex min-w-0 flex-1 items-center gap-1.5">
        {kind.taskId && <WorkerName taskId={kind.taskId} />}
        <span className="min-w-0 truncate">{machineWords(kind.machine, kind.command, mac ? "the Mac" : "the computer", kind.reason)}</span>
      </span>
    </div>
  );
};

/** "Decided: …", opening to why. */
const DecidedRow: FC<{ kind: DecidedStep }> = ({ kind }) => (
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

/**
 * A step of the lead that isn't work of its own: a team row, or a notice. Plumbing and what a
 * team sentence already says (a worker created, a report read) show nothing.
 */
export const ThreadStep: FC<{ step: BlockOrchestratorStep }> = ({ step }) => {
  const { kind } = step;
  if (kind.type === "machine") return <MachineRow kind={kind} />;
  if (kind.type === "decided") return <DecidedRow kind={kind} />;
  if (isTeamStep(kind)) return <TeamStep kind={kind} />;
  if (kind.type === "fullAccessSuggested") return <SandboxLimitNotice reason={kind.reason} />;
  if (kind.type === "merged") {
    return (
      <div data-slot="orchestrator-step" data-kind="merged" className={ROW}>
        <Branch aria-hidden className="size-4 shrink-0" />
        <span className="min-w-0 truncate">
          Merged {kind.branch} into {kind.base}
        </span>
      </div>
    );
  }
  return null;
};
