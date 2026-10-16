import { Stop, Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { unwrapCommand } from "@/app/conversation/activity/words";
import { ROW_DETAIL } from "@/components/assistant-ui/elements/activity-row";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import type { BackgroundRun, ItemStatus } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { cn } from "@/lib/utils";

/**
 * A command of the thread's that went on in the background, as its row: "Started background
 * terminal with npm run dev" while it runs, "Background terminal finished", "Background terminal
 * stopped with npm run dev" (with a stop square). It opens to its Shell box: its output once it
 * ended, "No output yet" while it runs.
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
      return "Background terminal finished";
    case "stopped":
    case "interrupted":
      return `Background terminal stopped with ${command}`;
  }
}

/** How a finished run ended, as its Shell box's last line reads it: "exit 1" is exit code 1. */
function ending(run: BackgroundRun): { status: ItemStatus; exit: number | null; stopped: boolean } {
  switch (run.state.type) {
    case "running":
      return { status: "inProgress", exit: null, stopped: false };
    case "finished": {
      const code = /exit(?: code)?\s+(-?\d+)/i.exec(run.state.status);
      return { status: code && code[1] !== "0" ? "failed" : "completed", exit: code ? Number(code[1]) : null, stopped: false };
    }
    case "stopped":
    case "interrupted":
      return { status: "failed", exit: null, stopped: true };
  }
}

/** A run's output, read once it ended (and again if its state changes). */
function RunOutput({ run }: { run: BackgroundRun }) {
  const [output, setOutput] = useState<{ for: string; text: string | null; error: string | null } | null>(null);
  const key = `${run.id}|${run.state.type}`;
  useEffect(() => {
    if (run.state.type === "running") return;
    let current = true;
    request({ method: "readBackgroundRun", conversationId: run.conversationId, runId: run.id }).then(
      ({ output: text }) => current && setOutput({ for: key, text, error: null }),
      (cause: unknown) => current && setOutput({ for: key, text: null, error: cause instanceof Error ? cause.message : String(cause) }),
    );
    return () => {
      current = false;
    };
  }, [run.conversationId, run.id, run.state.type, key]);
  const end = ending(run);
  if (run.state.type === "running") {
    return (
      <div className="flex flex-col gap-1">
        <ShellBox command={run.command} output={null} status="inProgress" exit={null} />
        <p className="text-muted-foreground text-sm">No output yet</p>
      </div>
    );
  }
  if (!output || output.for !== key) return <span className="shimmer text-xs">Loading</span>;
  if (output.error) return <p className="text-destructive text-xs">{output.error}</p>;
  return <ShellBox command={run.command} output={output.text} status={end.status} exit={end.exit} stopped={end.stopped} />;
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
