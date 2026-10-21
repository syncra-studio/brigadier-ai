import type { FC } from "react";

import { WorkerName } from "@/app/conversation/activity/WorkerName";
import type { MachineWords } from "@/app/conversation/blocks";
import { machineWords } from "@/app/conversation/rowWords";
import { ROW } from "@/components/conversation";
import { Clock, PauseCircle, PlayCircle } from "@/components/icons";
import { useApp } from "@/state/store";

const MACHINE_ICONS: Record<MachineWords["machine"], FC<{ className?: string }>> = {
  waitingToCool: Clock,
  waitingForBuild: Clock,
  paused: PauseCircle,
  resumed: PlayCircle,
};

/** A row about the machine; it names the worker when it is about one. */
export const MachineRow: FC<{ kind: MachineWords }> = ({ kind }) => {
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
