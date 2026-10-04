import { useCallback, useEffect, useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { RANKINGS_POLL_MS, refreshRunning } from "@/app/routing/rankings";
import { request } from "@/ipc/client";
import type { RankingsRefresh } from "@/ipc/generated";
import { useApp } from "@/state/store";

/** The rankings refresh the Routing page shows, with its actions. */
export type RankingsRefreshView = {
  refresh: RankingsRefresh | null;
  /** Why reading the refresh failed. */
  error: string | null;
  /** Asking for a refresh: busy while asked, and why it failed. */
  starting: ReturnType<typeof useAction>;
  start: () => Promise<void>;
  reset: () => Promise<void>;
};

/**
 * The rankings refresh as the Routing page shows it: read on opening the page and on every
 * rankings change, then every two seconds while a refresh runs (and the window can be seen).
 * `start` asks for a refresh (or joins the one running); `reset` drops the researched ratings,
 * stopping a running refresh first.
 */
export function useRankingsRefresh(): RankingsRefreshView {
  const connected = useApp((s) => s.connection.status === "connected");
  const revision = useApp((s) => s.rankingsRevision);
  const [refresh, setRefresh] = useState<RankingsRefresh | null>(null);
  const [error, setError] = useState<string | null>(null);
  const starting = useAction();
  // Only the latest read's answer is kept; an older one arriving late is dropped.
  const asked = useRef(0);

  const read = useCallback(() => {
    const seq = ++asked.current;
    return request({ method: "getRankingsRefresh" })
      .then(({ refresh: next }) => {
        if (seq !== asked.current) return;
        setRefresh(next);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (seq === asked.current) setError(cause instanceof Error ? cause.message : String(cause));
      });
  }, []);

  // Read on opening the page, on reconnecting and on every rankings change.
  useEffect(() => {
    if (connected) void read();
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [connected, revision, read]);

  const running = refresh !== null && refreshRunning(refresh.state);
  useEffect(() => {
    if (!connected || !running) return;
    const timer = window.setInterval(() => {
      if (useApp.getState().windowVisible) void read();
    }, RANKINGS_POLL_MS);
    return () => window.clearInterval(timer);
  }, [connected, running, read]);

  const start = useCallback(async () => {
    await request({ method: "refreshRankings" });
    await read();
  }, [read]);

  const reset = useCallback(async () => {
    const seq = ++asked.current;
    const { refresh: next } = await request({ method: "resetRankings" });
    if (seq === asked.current) setRefresh(next);
  }, []);

  return { refresh, error, starting, start, reset };
}
