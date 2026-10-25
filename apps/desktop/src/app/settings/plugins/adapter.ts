import { create } from "zustand";

import type { ProviderKind } from "@/ipc/generated";
import type { PluginItemDetails, PluginKind, PluginsView } from "./types";

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

/** The page's boundary to the daemon; fixtures supply the same data and actions. */
export type PluginsAdapter = {
  read: () => Promise<PluginsView>;
  /** Calls `onChange` when the page may have changed (the agents' setup, the projects). */
  subscribe: (onChange: () => void) => () => void;
  /** The card's switch: the whole item, in every agent, keeping where it's set to be. */
  switchPlugin: (key: string, on: boolean) => Promise<PluginAgentResult[]>;
  /** Where it's on: every project (null), or only these projects (never none). */
  setScope: (key: string, projectIds: string[] | null) => Promise<PluginAgentResult[]>;
  /** Share with every agent: an item only some agents have goes to the rest. */
  share: (key: string) => Promise<PluginAgentResult[]>;
  /** What a paste would add; `needsName` when it has no name of its own (a bare server, an address). */
  checkAdd: (text: string, name: string | null) => Promise<{ drafts: PluginDraft[]; needsName: boolean }>;
  /** Adds it to every agent, in every project (`projectIds` null) or these; `values`: env or headers typed in Advanced. */
  add: (text: string, name: string | null, projectIds: string[] | null, values: Record<string, string> | null) => Promise<PluginAgentResult[]>;
  /** Takes out what Brigadier added (its `removal`); an original stays as it is. */
  remove: (key: string) => Promise<PluginAgentResult[]>;
  update: (key: string) => Promise<PluginAgentResult[]>;
  /** What Open shows: a skill's SKILL.md files, a server's settings. */
  details: (key: string) => Promise<PluginItemDetails>;
  /**
   * Writes one of a skill's SKILL.md files, if it still holds `expected` (the text as opened);
   * the copies follow it. "changed" when the file changed since: nothing is written.
   */
  saveSkill: (key: string, path: string, contents: string, expected: string) => Promise<"saved" | "changed">;
  /**
   * Signs each agent in to a server in turn, opening the browser for the next: `serverId` and
   * `projectId` from its problem. Absent while the daemon can't: the page offers no Sign in then.
   */
  signIn?: (key: string, serverId: string, projectId: string | null) => Promise<PluginAgentResult[]>;
  cancelSignIn?: (key: string) => Promise<void>;
  /** A folder to add a skill from, or null when the user cancels. */
  pickFolder: () => Promise<string | null>;
  /** Trusts the project's folder, in Brigadier and in each agent's own settings (a project's note asks for it). */
  trustProject: (projectId: string) => Promise<void>;
};

const EMPTY: PluginsView = { projects: [], items: [], elsewhere: [] };

/** The page's data: no example items enter the app; the live adapter fills it. */
export const usePlugins = create<{
  view: PluginsView;
  adapter: PluginsAdapter | null;
  error: string | null;
  /** Reads the page again now. */
  refresh: () => void;
  /** Reads the page again, done once it's read. */
  reload: () => Promise<void>;
}>(() => ({ view: EMPTY, adapter: null, error: null, refresh: () => {}, reload: async () => {} }));

/** Installs `adapter` and follows its changes; a read older than the last one, or for an old connection, is dropped. */
export function connectPlugins(adapter: PluginsAdapter): () => void {
  let active = true;
  let generation = 0;
  const refresh = async () => {
    const current = ++generation;
    try {
      const view = await adapter.read();
      if (active && current === generation) usePlugins.setState({ view, error: null });
    } catch (cause) {
      if (active && current === generation) usePlugins.setState({ error: cause instanceof Error ? cause.message : String(cause) });
    }
  };
  usePlugins.setState({
    adapter,
    refresh: () => void refresh(),
    reload: refresh,
  });
  const unsubscribe = adapter.subscribe(() => void refresh());
  void refresh();
  return () => {
    active = false;
    unsubscribe();
    if (usePlugins.getState().adapter === adapter) usePlugins.setState({ adapter: null, refresh: () => {}, reload: async () => {} });
  };
}
