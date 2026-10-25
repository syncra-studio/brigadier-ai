import type { PluginKind, PluginProjectView, ProviderKind } from "@/ipc/generated";

/*
 * The Plugins page's view and requests as the daemon's shared-plugins protocol has them
 * (crates/ipc `plugins.rs`, `protocol.rs`), written out here until ts-rs generates them: then
 * each type below becomes a re-export from "@/ipc/generated" and `NextRequest` goes, nothing
 * else in the page changes.
 */

export type { PluginKind, PluginProjectView };

/** Where an item is on, or would be once switched on. */
export type PluginScope =
  | { type: "allProjects" }
  /** `projectIds`: the user's projects; `folders`: other folders that aren't projects, by path. */
  | { type: "someProjects"; projectIds: string[]; folders: string[] };

export type OnlyInReason = "ownPlugin" | "removed" | "cantShare" | "failed";

/** What needs the user; `message` is the card's line. */
export type PluginProblem =
  /** `serverId` and `projectId` go to the sign-in. */
  | { type: "needsSignIn"; message: string; serverId: string; projectId: string | null }
  | { type: "wontStart"; message: string }
  | { type: "updateReady"; message: string }
  /** Two different setups go by its name ("Two different setups named github"). */
  | { type: "conflict"; message: string }
  /** Only some agents have it. */
  | { type: "onlyIn"; message: string; agents: ProviderKind[]; reason: OnlyInReason };

/** What Remove takes: everything (Brigadier added it), only Brigadier's copies, or nothing (no Remove). */
export type PluginRemoval = "everything" | "copies" | "none";

/** One skill or MCP server, across every agent. */
export type PluginItemView = {
  key: string;
  kind: PluginKind;
  name: string;
  /** Empty when it has none: the page shows `detail`. */
  description: string;
  /** Its command line, address, or folder. */
  detail: string;
  /** A remote server's address. */
  url: string | null;
  /** Its site's icon, fetched by the daemon, as a data: URL. */
  icon: string | null;
  on: boolean;
  scope: PluginScope;
  problem: PluginProblem | null;
  /** Why the switch and the scope can't change it. */
  locked: string | null;
  removal: PluginRemoval;
  /** Share with every agent is offered (removed earlier, or a share that failed). */
  canShare: boolean;
  /** For a CLI plugin's group, how many skills it holds; 1 for a skill, 0 for a server. */
  skillCount: number;
  /** The files it lives in: the original's first, then the Library copy's. */
  files: string[];
};

/** A server only another app has (Claude Desktop, Cursor). */
export type PluginElsewhereView = { key: string; name: string; detail: string; url: string | null; foundIn: string; file: string };

export type PluginsView = {
  projects: PluginProjectView[];
  /** In order: problems first, then A–Z. */
  items: PluginItemView[];
  elsewhere: PluginElsewhereView[];
};

/** A skill's SKILL.md, for Open. */
export type PluginSkillFile = {
  name: string;
  /** Absolute. */
  path: string;
  contents: string;
  /** Why it can't be saved; null when it can. */
  readOnly: string | null;
};

/** A server's settings, for Open: names only, never a value. */
export type PluginServerSettings =
  | { type: "stdio"; command: string; args: string[]; cwd: string | null; envNames: string[] }
  | { type: "http"; url: string; headerNames: string[] };

export type PluginItemDetails = {
  key: string;
  kind: PluginKind;
  /** One for a skill, each one for a CLI plugin's group. */
  skills: PluginSkillFile[];
  server: PluginServerSettings | null;
};

/** The protocol's new and changed requests, until the generated `Request` has them. */
export type NextRequest =
  | { method: "getPlugins" }
  | { method: "switchPlugin"; key: string; on: boolean }
  | { method: "setPluginScope"; key: string; projectIds: string[] | null }
  | { method: "sharePlugin"; key: string }
  | { method: "removePlugin"; key: string }
  | { method: "updatePlugin"; key: string }
  | { method: "getPluginItem"; key: string }
  /** `expected`: the text as it was opened; the save is refused if the file changed since. */
  | { method: "savePluginSkill"; key: string; path: string; contents: string; expected: string }
  | { method: "addPlugin"; text: string; name: string | null; projectIds: string[] | null; values?: Record<string, string> };
