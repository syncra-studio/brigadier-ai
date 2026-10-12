import { create } from "zustand";

import { request } from "@/ipc/client";
import type { ProjectCandidate, ProviderKind, TerminalInfo } from "@/ipc/generated";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/**
 * The first-run setup: the coding agents (install, sign in), then the projects to add. It
 * shows until finished (it can't be skipped), and again when asked for from Settings.
 */

export const useOnboarding = create<{ reopened: boolean }>(() => ({ reopened: false }));

export function reopenOnboarding(): void {
  useOnboarding.setState({ reopened: true });
}

/** Closes the setup for good (Settings can bring it back). */
export async function finishOnboarding(): Promise<void> {
  useOnboarding.setState({ reopened: false });
  const settings = useApp.getState().settings;
  if (!settings.onboarded) await setSetting("onboarded", true);
}

/** The repositories the user's own Claude Code and Codex sessions worked in. */
export async function findProjects(): Promise<ProjectCandidate[]> {
  const { candidates } = await request({ method: "findProjects" });
  return candidates;
}

/** A terminal with the CLI's install or sign-in command typed, for the user to run. */
export async function openSetupTerminal(
  provider: ProviderKind,
  install: boolean,
  cols: number,
  rows: number,
): Promise<TerminalInfo> {
  const { terminal } = await request({ method: "openSetupTerminal", provider, install, cols, rows });
  return terminal;
}

export function closeSetupTerminal(terminalId: string): void {
  request({ method: "closeTerminal", terminalId }).catch((error: unknown) => {
    console.error("closing the setup terminal failed", error);
  });
}
