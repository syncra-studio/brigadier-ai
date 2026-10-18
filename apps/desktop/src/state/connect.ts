import { create } from "zustand";

import { request } from "@/ipc/client";
import type { ConnectProgress, ConnectView, EventEnvelope, ProviderKind } from "@/ipc/generated";

/**
 * Connect: the daemon installs an agent that is missing with its own installer, then opens its
 * own sign-in in the browser, publishing each step as `agentConnectChanged`; this holds the last
 * view, and why asking for it failed.
 */
export const useConnect = create<{
  view: ConnectView | null;
  /** Why asking to connect failed, by agent, until it is asked again. */
  errors: Partial<Record<ProviderKind, string>>;
}>(() => ({ view: null, errors: {} }));

/** Reads the daemon's view: on (re)connecting, after events may have been missed. */
export async function loadConnect(): Promise<void> {
  const { connect } = await request({ method: "getConnect" });
  useConnect.setState({ view: connect });
}

export function applyConnectEvents(batch: readonly EventEnvelope[]): void {
  for (const { event } of batch) {
    if (event.type === "agentConnectChanged") useConnect.setState({ view: event.connect });
  }
}

/** Where Connect is with `provider`: null when it never ran (or was cancelled). */
export function useConnectProgress(provider: ProviderKind): ConnectProgress | null {
  return useConnect((s) => s.view?.agents.find((agent) => agent.provider === provider)?.progress ?? null);
}

/** Whether it is installing or signing in now. */
export function connecting(progress: ConnectProgress | null): boolean {
  return progress?.type === "installing" || progress?.type === "signingIn";
}

/** Starts (or, after a failure, retries) connecting `provider`. */
export async function connectAgent(provider: ProviderKind): Promise<void> {
  useConnect.setState(({ errors }) => ({ errors: { ...errors, [provider]: undefined } }));
  try {
    await request({ method: "connectAgent", provider });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    useConnect.setState(({ errors }) => ({ errors: { ...errors, [provider]: message } }));
  }
}

export async function cancelConnect(provider: ProviderKind): Promise<void> {
  try {
    await request({ method: "cancelConnect", provider });
  } catch (error) {
    console.error("cancelling Connect failed", error);
  }
}
