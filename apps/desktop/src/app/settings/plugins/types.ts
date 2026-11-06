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
export type PluginRemoval = "everything" | "hide" | "copies" | "none";
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
  /** What its plugin also has that runs only on its own CLI, read-only. */
  cliOnly?: string[];
  signIn?: CatalogSignIn;
};
/** A skill or MCP server found since the last Refresh, off until the user adds it. */
export type PluginFoundView = {
  key: string;
  kind: PluginKind;
  name: string;
  detail: string;
  /** The agents that have it, in words: "Claude Code + Codex". */
  foundIn: string;
};
/** One whose files have left every agent: it has only a name now. */
export type PluginGoneView = { key: string; kind: PluginKind; name: string };
/** One the user kept out of Brigadier; its files stay, and Add can bring it back. */
export type PluginHiddenView = { key: string; kind: PluginKind; name: string; detail: string };
/** What a Refresh found. */
export type PluginChanges = {
  found: PluginFoundView[];
  gone: PluginGoneView[];
  /** Settings files that couldn't be read: nothing counts as gone while there are any. */
  unread: string[];
};
/** What the user chose for each: every key of a `PluginChanges` is in one of these. */
export type PluginChangeChoices = { add: string[]; decline: string[]; remove: string[]; keep: string[] };
export type PluginsView = {
  projects: PluginProjectView[];
  items: PluginItemView[];
  elsewhere: PluginElsewhereView[];
  /** Gone items the user chose to keep: they return with their settings if their files do. */
  missing?: PluginGoneView[];
  hidden?: PluginHiddenView[];
  /** What each CLI has of its own that only it can run, read-only. */
  cliOnly?: string[];
};
