import { RunOutput } from "@/app/conversation/activity/RunOutput";
import { unwrapCommand } from "@/app/conversation/activity/words";
import { ROW_DETAIL, ThreadActivity } from "@/components/conversation";
import { Stop, Terminal } from "@/components/icons";
import type { BackgroundRun } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/**
 * A command of the thread's that went on in the background, as its row: "Started background
 * terminal with npm run dev" while it runs, "Background terminal finished with npm run dev",
 * "Background terminal stopped with npm run dev" (with a stop square). It opens to its Shell
 * box: its output once it ended, "No output yet" while it runs.
 */

/** The command a row names: its first line. */
function commandLine(command: string): string {
  const [first = ""] = unwrapCommand(command).split("\n");
  return first.length > 80 ? `${first.slice(0, 79).trimEnd()}…` : first;
}

/** A background run's row words. */
export function backgroundWords(run: BackgroundRun): string {
  const command = commandLine(run.command);
  switch (run.state.type) {
    case "running":
      return `Started background terminal with ${command}`;
    case "finished":
      return `Background terminal finished with ${command}`;
    case "stopped":
    case "interrupted":
      return `Background terminal stopped with ${command}`;
  }
}

/** One background run's row. */
export function BackgroundRunRow({ run }: { run: BackgroundRun }) {
  const stopped = run.state.type === "stopped" || run.state.type === "interrupted";
  const Icon = stopped ? Stop : Terminal;
  return (
    <ThreadActivity
      data-slot="background-run-row"
      data-state={run.state.type}
      detail={<RunOutput run={run} />}
      detailClassName={cn(ROW_DETAIL, "ps-0")}
    >
      <Icon aria-hidden data-icon={stopped ? "stop" : "run"} className="size-4 shrink-0" />
      <span className="min-w-0 truncate">{backgroundWords(run)}</span>
    </ThreadActivity>
  );
}
