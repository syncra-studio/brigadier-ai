import { useEffect, useState } from "react";

import { ShellBox } from "@/app/conversation/activity/StepDetail";
import type { BackgroundRun, ItemStatus } from "@/ipc/generated";
import { request } from "@/ipc/client";

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
export function RunOutput({ run }: { run: BackgroundRun }) {
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
