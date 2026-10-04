import { useEffect } from "react";
import { create } from "zustand";

import { request } from "@/ipc/client";
import type { EventEnvelope, UsageView } from "@/ipc/generated";
import { useApp } from "@/state/store";

/**
 * The Usage and Models pages' data: one `getUsage` read, taken again when a provider is checked
 * and every minute while one of them is shown.
 */
export type UsageState = {
  view: UsageView | null;
  /** The project the models' learned adjustments are for; `null`: every project. */
  projectId: string | null;
  loading: boolean;
  error: string | null;
  /** A page showing it is mounted: provider checks read the view again. */
  shown: boolean;
};

export const useUsage = create<UsageState>()(() => ({
  view: null,
  projectId: null,
  loading: false,
  error: null,
  shown: false,
}));

let inFlight: Promise<void> | null = null;
let again = false;

/** Reads the view for the chosen project; a read asked for meanwhile runs once after it. */
export function loadUsage(): Promise<void> {
  if (inFlight) {
    again = true;
    return inFlight;
  }
  useUsage.setState({ loading: true });
  inFlight = (async () => {
    try {
      do {
        again = false;
        const projectId = useUsage.getState().projectId;
        try {
          const { usage } = await request({ method: "getUsage", projectId });
          // A project picked while reading is read next; keep this one only if it still applies.
          if (useUsage.getState().projectId === projectId) {
            useUsage.setState({ view: usage, error: null });
          }
        } catch (error) {
          useUsage.setState({ error: error instanceof Error ? error.message : String(error) });
        }
      } while (again);
    } finally {
      inFlight = null;
      useUsage.setState({ loading: false });
    }
  })();
  return inFlight;
}

export function setUsageProject(projectId: string | null): void {
  useUsage.setState({ projectId });
  void loadUsage();
}

export function setUsageShown(shown: boolean): void {
  useUsage.setState({ shown });
}

/**
 * A provider check changes windows, estimates and balancing, and a rankings change the models'
 * ratings: read the view again.
 */
export function applyUsageEvents(batch: readonly EventEnvelope[]): void {
  if (!useUsage.getState().shown) return;
  if (batch.some(({ event }) => event.type === "providerChecked" || event.type === "rankingsChanged")) {
    void loadUsage();
  }
}

/**
 * Asks the repository for a newer registry now, then reads the whole view again: a new
 * revision changes the models' strengths too.
 */
export async function checkRegistry(): Promise<void> {
  const { registry } = await request({ method: "checkRegistry" });
  const view = useUsage.getState().view;
  if (view) useUsage.setState({ view: { ...view, registry } });
  await loadUsage();
}

/** The view is read again this often while a page shows it and the window can be seen. */
const REFRESH_EVERY_MS = 60_000;

/** Reads the view on opening a page that shows it, then every minute while it can be seen. */
export function useUsageRefresh(): void {
  const connected = useApp((s) => s.connection.status === "connected");
  useEffect(() => {
    if (!connected) return;
    setUsageShown(true);
    void loadUsage();
    const timer = window.setInterval(() => {
      if (useApp.getState().windowVisible) void loadUsage();
    }, REFRESH_EVERY_MS);
    return () => {
      window.clearInterval(timer);
      setUsageShown(false);
    };
  }, [connected]);
}
