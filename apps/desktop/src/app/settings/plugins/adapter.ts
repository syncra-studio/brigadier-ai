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

/** The page's boundary to the daemon; fixtures supply the same data and actions. */
export type PluginsAdapter = {
  /** The page, looking at project `projectId`, else at every project. */
  read: (projectId: string | null) => Promise<PluginsView>;
  /** Calls `onChange` when the page may have changed (the agents' setup, the projects). */
  subscribe: (onChange: () => void) => () => void;
  /** The row switch: `projectId` null means every project. */
  switchPlugin: (key: string, projectId: string | null, on: boolean) => Promise<PluginAgentResult[]>;
  /** What a paste would add. */
  checkAdd: (text: string) => Promise<PluginDraft[]>;
  add: (text: string, name: string | null, projectId: string | null, shared: boolean) => Promise<PluginAgentResult[]>;
  /** Use everywhere: into the Library, for every agent. */
  everyAgent: (key: string) => Promise<PluginAgentResult[]>;
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
  setProject: (project: string | null) => void;
}>(() => ({ view: EMPTY, project: null, adapter: null, error: null, refresh: () => {}, setProject: () => {} }));

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
    if (usePlugins.getState().adapter === adapter) usePlugins.setState({ adapter: null, refresh: () => {}, setProject: () => {} });
  };
}
