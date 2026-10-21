import type { IntegrationCatalog, IntegrationSwitches } from "@/app/settings/integrations/types";
import type { McpSignInStatus } from "@/ipc/generated";
import { create } from "zustand";

/** The IPC/store boundary; fixtures supply the same data and actions as the live page. */
export type IntegrationSnapshot = {
  catalog: IntegrationCatalog;
  switches: IntegrationSwitches;
};

export type IntegrationAdapter = {
  /** GetIntegrations, scoped to a trusted project when one is selected. */
  read: (project?: string) => Promise<IntegrationSnapshot>;
  /** Config or settings changes ask the page to read a fresh snapshot. */
  subscribe: (onChange: () => void) => () => void;
  /** UpdateSettings edits the latest off-list; source configs remain read-only. */
  setServerEnabled: (id: string, enabled: boolean) => Promise<void>;
  setSkillEnabled: (id: string, enabled: boolean) => Promise<void>;
  showPath: (path: string) => Promise<void>;
  /** SignInMcpServer: signs in to server `id` once in Brigadier; answers at once with the sign-in's id. */
  signIn: (id: string) => Promise<string>;
  /** GetMcpSignIn: how sign-in `signInId` went. */
  signInStatus: (signInId: string) => Promise<McpSignInStatus>;
  cancelSignIn: (signInId: string) => Promise<void>;
  reopenSignIn: (signInId: string) => Promise<void>;
  /** SignOutMcpServer: removes Brigadier's own sign-in to server `id`. */
  signOut: (id: string) => Promise<void>;
  /** AskMcpSignInAgain: reads the agents' saved sign-ins to server `id` again, so macOS may ask. */
  askSignInAgain: (id: string) => Promise<void>;
};

const empty: IntegrationSnapshot = {
  catalog: { servers: [], skills: [] },
  switches: { mcpServersOff: [], skillsOff: [] },
};

/** No example integrations enter the app. The backend adapter installs this when available. */
export const useIntegrations = create<{
  snapshot: IntegrationSnapshot;
  adapter: IntegrationAdapter | null;
  error: string | null;
  /** Reads a fresh snapshot now, as a config change does. */
  refresh: () => void;
}>(() => ({ snapshot: empty, adapter: null, error: null, refresh: () => {} }));

/** Install the IPC boundary and follow config changes; discard reads from an older connection. */
export function connectIntegrations(adapter: IntegrationAdapter, project?: string): () => void {
  let active = true;
  let generation = 0;
  useIntegrations.setState({ adapter });
  const refresh = async () => {
    const current = ++generation;
    try {
      const snapshot = await adapter.read(project);
      if (active && current === generation) useIntegrations.setState({ snapshot, error: null });
    } catch (cause) {
      if (active && current === generation) useIntegrations.setState({ error: String(cause) });
    }
  };
  const unsubscribe = adapter.subscribe(() => void refresh());
  useIntegrations.setState({ refresh: () => void refresh() });
  void refresh();
  return () => {
    active = false;
    unsubscribe();
    if (useIntegrations.getState().adapter === adapter) useIntegrations.setState({ adapter: null, refresh: () => {} });
  };
}
