import { create } from "zustand";

import { request } from "@/ipc/client";
import type { ProjectCandidate } from "@/ipc/generated";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/**
 * The first-run setup: the coding agents (install, sign in), then the projects to add. It
 * shows until finished (it can't be skipped), and again when asked for from Settings.
 */

export const useOnboarding = create<{
  reopened: boolean;
  /** Its finish is being saved: the setup stays open (and on its step) until it is. */
  finishing: boolean;
}>(() => ({ reopened: false, finishing: false }));

export function reopenOnboarding(): void {
  useOnboarding.setState({ reopened: true });
}

/**
 * Closes the setup for good (Settings can bring it back). A first run's closes once that is
 * saved; when it can't be, the setup stays where it was, for the caller to say why.
 */
export async function finishOnboarding(): Promise<void> {
  useOnboarding.setState({ reopened: false });
  const settings = useApp.getState().settings;
  if (settings.onboarded) return;
  useOnboarding.setState({ finishing: true });
  try {
    await setSetting("onboarded", true);
  } finally {
    useOnboarding.setState({ finishing: false });
  }
}

/** The repositories the user's own Claude Code and Codex sessions worked in. */
export async function findProjects(): Promise<ProjectCandidate[]> {
  const { candidates } = await request({ method: "findProjects" });
  return candidates;
}

export function closeSetupTerminal(terminalId: string): void {
  request({ method: "closeTerminal", terminalId }).catch((error: unknown) => {
    console.error("closing the setup terminal failed", error);
  });
}
