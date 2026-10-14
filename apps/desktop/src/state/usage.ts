import { useEffect } from "react";
import { create } from "zustand";

import { request } from "@/ipc/client";
import type { AccountView, EventEnvelope, UsageActivity, UsageView } from "@/ipc/generated";
import { refreshAccounts } from "@/state/accounts";
import { useApp } from "@/state/store";

/**
 * The Usage and Models pages' data: one `getUsage` read, taken again when a provider or an
 * account is checked and every minute while one of them is shown; the Usage page's Activity
 * (`getUsageActivity`) over the days it shows; and its Refresh.
 */
export type UsageState = {
  view: UsageView | null;
  /** The project the models' learned adjustments are for; `null`: every project. */
  projectId: string | null;
  loading: boolean;
  error: string | null;
  /** A page showing it is mounted: provider checks read the view again. */
  shown: boolean;
  /** The Activity tab's read, and the days it covers. */
  activity: UsageActivity | null;
  days: ActivityDays;
  /** The Activity tab shows: checks read it again too. */
  activityShown: boolean;
  /** Refresh is checking every account again. */
  refreshing: boolean;
  /** What went wrong with the last Refresh. */
  refreshError: string | null;
};

/** The ranges Activity shows. */
export type ActivityDays = 7 | 30;

export const useUsage = create<UsageState>()(() => ({
  view: null,
  projectId: null,
  loading: false,
  error: null,
  shown: false,
  activity: null,
  days: 7,
  activityShown: false,
  refreshing: false,
  refreshError: null,
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
          useUsage.setState({ error: errorText(error) });
        }
      } while (again);
    } finally {
      inFlight = null;
      useUsage.setState({ loading: false });
    }
  })();
  return inFlight;
}

/**
 * Reads the view ahead of a page that shows it, when there is none yet: not while a read is on
 * its way, or without the daemon.
 */
export function prefetchUsage(): void {
  if (useUsage.getState().view || inFlight || useApp.getState().connection.status !== "connected") return;
  void loadUsage();
}

/** Reads Activity over the days chosen; a range chosen meanwhile is kept only when it applies. */
export async function loadActivity(): Promise<void> {
  const days = useUsage.getState().days;
  try {
    const { activity } = await request({ method: "getUsageActivity", days });
    if (useUsage.getState().days === days) useUsage.setState({ activity });
  } catch (error) {
    useUsage.setState({ error: errorText(error) });
  }
}

export function setActivityDays(days: ActivityDays): void {
  useUsage.setState({ days, activity: null });
  void loadActivity();
}

export function setActivityShown(shown: boolean): void {
  useUsage.setState({ activityShown: shown });
  if (shown) void loadActivity();
}

export function setUsageProject(projectId: string | null): void {
  useUsage.setState({ projectId });
  void loadUsage();
}

export function setUsageShown(shown: boolean): void {
  useUsage.setState({ shown });
}

/** The longest Refresh waits for every account to be checked. */
export const REFRESH_LIMIT_MS = 60_000;

/** Whether every account was checked afresh for the refresh `generation`. */
export function checkedFor(accounts: readonly AccountView[], generation: number): boolean {
  return accounts.every((view) => !view.checking && view.checkedGeneration >= generation);
}

/**
 * Checks every account again, then reads the view: done once each account says it was checked
 * for this refresh (a check that was already running when it was asked runs again, and
 * doesn't count), or, past `REFRESH_LIMIT_MS`, with a line saying it didn't finish.
 */
export async function refreshUsage(limitMs = REFRESH_LIMIT_MS): Promise<void> {
  if (useUsage.getState().refreshing) return;
  useUsage.setState({ refreshing: true, refreshError: null });
  try {
    const generation = await refreshAccounts();
    if (!(await accountsChecked(generation, limitMs))) {
      useUsage.setState({ refreshError: "Not every account answered in time; some readings may be old." });
    }
    await Promise.all([loadUsage(), useUsage.getState().activityShown ? loadActivity() : null]);
  } catch (error) {
    useUsage.setState({ refreshError: `Couldn't check the accounts: ${errorText(error)}` });
  } finally {
    useUsage.setState({ refreshing: false });
  }
}

/** Waits until every account was checked for `generation`; false past `limitMs`. */
function accountsChecked(generation: number, limitMs: number): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    const finish = (done: boolean) => {
      unsubscribe();
      clearTimeout(timer);
      resolve(done);
    };
    const check = () => {
      const accounts = useApp.getState().accounts?.accounts;
      if (accounts && checkedFor(accounts, generation)) finish(true);
    };
    const unsubscribe = useApp.subscribe(check);
    const timer = setTimeout(() => finish(false), limitMs);
    check();
  });
}

/**
 * A provider or account check changes windows, estimates and balancing, and a rankings change
 * the models' ratings: read the view (and Activity, if shown) again.
 */
export function applyUsageEvents(batch: readonly EventEnvelope[]): void {
  const state = useUsage.getState();
  if (!state.shown) return;
  if (
    batch.some(({ event }) =>
      ["providerChecked", "accountsChecked", "rankingsChanged"].includes(event.type),
    )
  ) {
    void loadUsage();
    if (state.activityShown) void loadActivity();
  }
}

/** The view is read again this often while a page shows it and the window can be seen. */
const REFRESH_EVERY_MS = 60_000;

/** Reads the view on opening a page that shows it, then every minute while it can be seen. */
export function useUsageRefresh(): void {
  const connected = useApp((s) => s.connection.status === "connected");
  useEffect(() => {
    if (!connected) return;
    setUsageShown(true);
    // A read already on its way (a prefetch) answers for the page.
    if (!inFlight) void loadUsage();
    const timer = window.setInterval(() => {
      if (!useApp.getState().windowVisible) return;
      void loadUsage();
      if (useUsage.getState().activityShown) void loadActivity();
    }, REFRESH_EVERY_MS);
    return () => {
      window.clearInterval(timer);
      setUsageShown(false);
    };
  }, [connected]);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
