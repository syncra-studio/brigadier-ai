/** The Plugins page's view of the daemon's catalog (`live.ts` maps `IntegrationsView` to it). */
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
  transport: { kind: "stdio"; command: string; args: string[] } | { kind: "http"; url: string };
  env: string[];
  headers: string[];
  signIn: Partial<Record<IntegrationAgent, IntegrationSignIn>>;
  /** The sign-in in Brigadier waiting for the user's browser, by its id. */
  pendingSignIn?: string;
  nativeAgents: IntegrationAgent[];
  canSwitchOff: boolean;
  error?: string;
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
  canSwitchOff: boolean;
};

export type IntegrationCatalog = {
  servers: IntegrationServer[];
  skills: IntegrationSkill[];
  trustedProjectCount?: number;
  untrustedProjects?: string[];
};

export type IntegrationSwitches = {
  mcpServersOff: string[];
  skillsOff: string[];
};
