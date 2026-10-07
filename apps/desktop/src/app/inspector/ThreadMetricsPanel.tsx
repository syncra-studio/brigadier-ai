import { useEffect, useRef, useState } from "react";

import { editsLine, growthRows } from "@/app/inspector/threadMetrics";
import { request } from "@/ipc/client";
import type { ThreadMetrics } from "@/ipc/generated";
import { formatTokens } from "@/lib/format";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/** The metrics are read again this often while shown and the window can be seen. */
const READ_EVERY_MS = 15_000;
/** Requests listed, newest first. */
const SHOWN_REQUESTS = 8;

/**
 * How the session's thread works (THREAD-PLAN.md Q13): the commits it made itself, and how
 * its context grew over each request. Read on opening, when its run state changes, and every
 * few seconds while the window can be seen.
 */
export function ThreadMetricsPanel({ sessionId }: { sessionId: string }) {
  const connected = useApp((s) => s.connection.status === "connected");
  const run = useBoard((s) => (s.board?.conversationId === sessionId ? s.board.run : null));
  const [metrics, setMetrics] = useState<ThreadMetrics | null>(null);
  const [error, setError] = useState<string | null>(null);
  const asked = useRef(0);
  useEffect(() => {
    if (!connected) return undefined;
    const read = () => {
      const seq = ++asked.current;
      request({ method: "getThreadMetrics", conversationId: sessionId })
        .then(({ metrics: next }) => {
          if (seq !== asked.current) return;
          setMetrics(next);
          setError(null);
        })
        .catch((cause: unknown) => {
          if (seq === asked.current) setError(cause instanceof Error ? cause.message : String(cause));
        });
    };
    read();
    const timer = window.setInterval(() => {
      if (useApp.getState().windowVisible) read();
    }, READ_EVERY_MS);
    return () => window.clearInterval(timer);
    // Read again as the thread's run starts or ends: its context and commits move then.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [connected, sessionId, run]);

  const shown = metrics?.conversationId === sessionId ? metrics : null;
  const rows = shown ? growthRows(shown.requests, SHOWN_REQUESTS) : [];
  return (
    <section aria-label="Thread metrics" className="flex flex-col gap-1">
      <div className="flex items-baseline gap-2">
        <span className="flex-1 font-medium">Thread</span>
        <span className="text-muted-foreground tabular-nums">
          {shown?.contextTokens != null && `context now ${formatTokens(shown.contextTokens)} tokens`}
        </span>
      </div>
      {error ? (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      ) : (
        <span className="text-muted-foreground">
          {shown ? `Own edits: ${editsLine(shown.edits)}` : "Loading…"}
        </span>
      )}
      {rows.length > 0 && (
        <ul className="flex flex-col" aria-label="Context per request">
          {rows.map((row) => (
            <li key={row.key} className="flex items-baseline gap-2 tabular-nums">
              <span className="text-foreground min-w-0 flex-1 truncate" title={row.label}>
                {row.label}
              </span>
              <span className="text-muted-foreground shrink-0">{row.calls}</span>
              <span className="text-muted-foreground shrink-0">{row.span}</span>
              <span className="w-16 shrink-0 text-end">{row.growth}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
