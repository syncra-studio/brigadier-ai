import { create } from "zustand";

import type { PluginKind, PluginsView, ProviderKind } from "@/ipc/generated";

/** Something read from what the user pasted, before it is added. */
export type PluginDraft = {
  name: string;
  kind: PluginKind;
  /** Its command, address or folder. */
  detail: string;
  /** Reached over the network. */
  remote: boolean;
  /** Signs in with the user's browser. */
  signsIn: boolean;
  /** The names of the values it is given (env, headers); their values stay in Brigadier. */
  secrets: string[];
  /** Why it can't be added, in words for the user. */
  error: string | null;
};

/** How one agent took a change. */
export type PluginAgentResult = {
  agent: ProviderKind;
  ok: boolean;
  /** Why it didn't (or what to know although it did), in words for the user. */
  message: string | null;
};

/** Add's Advanced beyond the name and where. */
export type PluginAddMore = {
  /** Only these agents get it; null: every agent. */
  agents: ProviderKind[] | null;
  /** The one server's env (or headers) the user typed, by name: they go to the Keychain. */
  values: Record<string, string> | null;
};

/** The page's boundary to the daemon; fixtures supply the same data and actions. */
export type PluginsAdapter = {
  /** The page, looking at project `projectId`, else at every project. */
  read: (projectId: string | null) => Promise<PluginsView>;
  /** Calls `onChange` when the page may have changed (the agents' setup, the projects). */
  subscribe: (onChange: () => void) => () => void;
  /** The row switch: `projectId` null means every project; `agent`, for that agent alone. */
  switchPlugin: (key: string, projectId: string | null, on: boolean, agent?: ProviderKind) => Promise<PluginAgentResult[]>;
  /**
   * The place picker: a Library item on in every project (`projectIds` null), or in exactly these
   * projects. `undo`: what its Undo takes, each agent back exactly where it had it.
   */
  setPlaces: (key: string, projectIds: string[] | null) => Promise<{ results: PluginAgentResult[]; undo: string | null }>;
  undoPlaces: (undo: string) => Promise<PluginAgentResult[]>;
  /** Add to every agent, for a Library item: the agents missing it get it where the others have it. */
  addMissing: (key: string) => Promise<PluginAgentResult[]>;
  /** What a paste would add; `needsName` when it has no name of its own (a bare server, an address). */
  checkAdd: (text: string, name: string | null) => Promise<{ drafts: PluginDraft[]; needsName: boolean }>;
  add: (text: string, name: string | null, projectId: string | null, shared: boolean, more: PluginAddMore) => Promise<PluginAgentResult[]>;
  /** Add to every agent: into the Library, for every agent. `key`: the Library's copy, what Undo takes. */
  everyAgent: (key: string) => Promise<{ results: PluginAgentResult[]; key: string | null }>;
  undoEveryAgent: (key: string) => Promise<PluginAgentResult[]>;
  remove: (key: string) => Promise<PluginAgentResult[]>;
  update: (key: string) => Promise<PluginAgentResult[]>;
  /**
   * Signs each agent in to the item's server in turn, opening the browser for the next. Absent
   * while the daemon can't: the page offers no Sign in then.
   */
  signIn?: (key: string, projectId: string | null) => Promise<PluginAgentResult[]>;
  cancelSignIn?: (key: string) => Promise<void>;
  /** A folder to add a skill from, or null when the user cancels. */
  pickFolder: () => Promise<string | null>;
  /** Trusts the project's folder, in Brigadier and in each agent's own settings. */
  trustProject: (projectId: string) => Promise<void>;
  /** Opens Add project. */
  addProject: () => void;
};

const EMPTY: PluginsView = { agents: [], projects: [], items: [], elsewhere: [] };

/** The page's data: no example items enter the app; the live adapter fills it. */
export const usePlugins = create<{
  view: PluginsView;
  /** The project the filter picks, by id; null for All projects. */
  project: string | null;
  adapter: PluginsAdapter | null;
  error: string | null;
  /** Reads the page again now. */
  refresh: () => void;
  /** Reads the page again, done once it's read. */
  reload: () => Promise<void>;
  setProject: (project: string | null) => void;
}>(() => ({ view: EMPTY, project: null, adapter: null, error: null, refresh: () => {}, reload: async () => {}, setProject: () => {} }));

/** Installs `adapter` and follows its changes; a read for an older project or connection is dropped. */
export function connectPlugins(adapter: PluginsAdapter): () => void {
  let active = true;
  let generation = 0;
  const refresh = async () => {
    const current = ++generation;
    const { project } = usePlugins.getState();
    try {
      const view = await adapter.read(project);
      if (active && current === generation) usePlugins.setState({ view, error: null });
    } catch (cause) {
      if (active && current === generation) usePlugins.setState({ error: cause instanceof Error ? cause.message : String(cause) });
    }
  };
  usePlugins.setState({
    adapter,
    refresh: () => void refresh(),
    reload: refresh,
    setProject: (project) => {
      usePlugins.setState({ project });
      void refresh();
    },
  });
  const unsubscribe = adapter.subscribe(() => void refresh());
  void refresh();
  return () => {
    active = false;
    unsubscribe();
    if (usePlugins.getState().adapter === adapter) usePlugins.setState({ adapter: null, refresh: () => {}, reload: async () => {}, setProject: () => {} });
  };
}
