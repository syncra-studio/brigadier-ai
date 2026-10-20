import { useEffect, useState } from "react";

import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { unwrapCommand } from "@/app/conversation/activity/words";
import { ROW_DETAIL, ThreadActivity } from "@/components/conversation";
import { Stop, Terminal } from "@/components/icons";
import type { BackgroundRun, ItemStatus } from "@/ipc/generated";
import { request } from "@/ipc/client";
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

/** While a run goes on, its output is read again this often (the row's box is open). */
const READ_EVERY_MS = 2000;

/** A run's output: read when its box opens, again every 2 s while it runs, and once more when it ends. */
function RunOutput({ run }: { run: BackgroundRun }) {
  const [output, setOutput] = useState<{ for: string; text: string | null; error: string | null } | null>(null);
  const running = run.state.type === "running";
  const key = `${run.id}|${run.state.type}`;
  useEffect(() => {
    let current = true;
    const read = () =>
      request({ method: "readBackgroundRun", conversationId: run.conversationId, runId: run.id }).then(
        ({ output: text }) => current && setOutput({ for: key, text, error: null }),
        (cause: unknown) => current && setOutput({ for: key, text: null, error: cause instanceof Error ? cause.message : String(cause) }),
      );
    void read();
    const timer = running ? window.setInterval(() => void read(), READ_EVERY_MS) : null;
    return () => {
      current = false;
      if (timer !== null) window.clearInterval(timer);
    };
  }, [run.conversationId, run.id, key, running]);
  const end = ending(run);
  // While it runs, what it printed so far stays until the next read: the box never blanks.
  const shown = output && (output.for === key || running) ? output : null;
  if (!shown && !running) return <span className="shimmer text-xs">Loading</span>;
  if (shown?.error && !running) return <p className="text-destructive text-xs">{shown.error}</p>;
  const text = shown?.text || null;
  return (
    <ShellBox
      command={run.command}
      output={text}
      status={end.status}
      exit={end.exit}
      stopped={end.stopped}
      notes={running && !text ? ["No output yet"] : []}
    />
  );
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
