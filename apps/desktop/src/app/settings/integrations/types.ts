/** The Plugins page's view of the daemon's catalog (`live.ts` maps `IntegrationsView` to it). */
import type { ProjectSwitches } from "@/ipc/generated";

export type IntegrationAgent = "claude" | "codex";

export type IntegrationSource =
  | "ClaudeUser"
  | "ClaudeLocal"
  | "ClaudeProjectFile"
  | "CodexUser"
  | "CodexProject"
  | "Desktop"
  | "CursorUser"
  | "CursorProject";

/** How one agent's workers are signed in to a remote server (`SignInState`). */
export type IntegrationSignIn =
  | { kind: "native" }
  | { kind: "reusable"; from: IntegrationAgent }
  | { kind: "brigadier"; sinceMs?: number }
  | { kind: "needed" }
  | { kind: "unknown" }
  | { kind: "notAllowed"; from: IntegrationAgent }
  | { kind: "unsupported" };

export type IntegrationServer = {
  id: string;
  name: string;
  sources: IntegrationSource[];
  project?: string;
  transport: { kind: "stdio"; command: string; args: string[]; /** The folder it runs in. */ cwd?: string } | { kind: "http"; url: string };
  env: string[];
  headers: string[];
  signIn: Partial<Record<IntegrationAgent, IntegrationSignIn>>;
  /** The sign-in in Brigadier waiting for the user's browser, by its id. */
  pendingSignIn?: string;
  nativeAgents: IntegrationAgent[];
  canSwitchOff: boolean;
  error?: string;
  /** Every daemon id behind the row (copies of one server): each switch covers them all. */
  ids?: string[];
  /** A project's own server: its project's id and folder. */
  projectId?: string;
  projectPath?: string;
  sourceFiles?: { source: IntegrationSource; path: string; line?: number }[];
};

export type IntegrationSkill = {
  id: string;
  name: string;
  description: string;
  locations: string[];
  locationDetails?: { path: string; linkedTo?: string; agents: IntegrationAgent[] }[];
  nativeAgents: IntegrationAgent[];
  project?: string;
  projectId?: string;
  projectPath?: string;
  canSwitchOff: boolean;
  /** Claude finds it by itself, and Claude Code switches skills off by name. */
  claudeOffByName?: boolean;
};

/** One of the user's projects, for its exceptions: only a trusted one's folder is read. */
export type IntegrationProject = { id: string; name: string; path: string; trusted: boolean };

export type IntegrationKind = "server" | "skill";

export type IntegrationCatalog = {
  servers: IntegrationServer[];
  skills: IntegrationSkill[];
  trustedProjectCount?: number;
  untrustedProjects?: string[];
  /** Every project with a folder, trusted or not. */
  projects?: IntegrationProject[];
  /**
   * Each trusted project's own catalog, by project id: what its sessions find (its nativeTo,
   * sign-ins and folders), with the other projects' items used in every project.
   */
  projectCatalogs?: Record<string, { servers: IntegrationServer[]; skills: IntegrationSkill[] }>;
};

export type IntegrationSwitches = {
  /** The rows off by default. */
  mcpServersOff: string[];
  skillsOff: string[];
  /** Settings' lists as they are, by daemon id. */
  projectSwitches?: ProjectSwitches[];
  serversEverywhere?: string[];
  skillsEverywhere?: string[];
};
