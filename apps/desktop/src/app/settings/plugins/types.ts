/** Page display shapes derived locally from the catalog, rather than daemon IPC views. */
import type { CatalogSignIn, PluginKind, ProviderKind } from "@/ipc/generated";
export type { PluginItemDetails, PluginKind, PluginServerSettings, PluginSkillFile } from "@/ipc/generated";

export type PluginProjectView = {
  id: string;
  name: string;
  path: string;
  notes: { agent: ProviderKind; message: string }[];
};
export type OnlyInReason = "ownPlugin" | "removed" | "cantShare" | "failed";
export type PluginElsewhereView = {
  key: string;
  name: string;
  detail: string;
  url: string | null;
  foundIn: string;
  file: string;
};
export type PluginProblem =
  | { type: "needsSignIn"; message: string; serverId: string; projectId: string | null }
  | { type: "wontStart" | "conflict" | "updateReady"; message: string }
  | { type: "onlyIn"; message: string; agents: ProviderKind[]; reason: OnlyInReason };
export type PluginRemoval = "everything" | "copies" | "none";
export type PluginScope = { type: "allProjects" } | { type: "someProjects"; projectIds: string[]; folders: string[] };
export type PluginItemView = {
  key: string;
  kind: PluginKind;
  name: string;
  description: string;
  detail: string;
  url: string | null;
  icon: string | null;
  on: boolean;
  scope: PluginScope;
  problem: PluginProblem | null;
  locked: string | null;
  removal: PluginRemoval;
  canShare: boolean;
  skillCount: number;
  files: string[];
  differentPerAgent?: boolean;
  signIn?: CatalogSignIn;
};
export type PluginsView = { projects: PluginProjectView[]; items: PluginItemView[]; elsewhere: PluginElsewhereView[] };
