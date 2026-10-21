import type {
  IntegrationCatalog,
  IntegrationKind,
  IntegrationProject,
  IntegrationServer,
  IntegrationSkill,
  IntegrationSwitches,
} from "@/app/settings/integrations/types";
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
  /** Turns a row's `ids` on or off in one project, or back to the default there (`null`). */
  setInProject: (kind: IntegrationKind, ids: string[], projectId: string, enabled: boolean | null) => Promise<void>;
  /** Uses a project's own item in every project (`on`), or keeps it in its project `origin` again. */
  setEverywhere: (kind: IntegrationKind, ids: string[], origin: string, on: boolean) => Promise<void>;
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

/** Where a row is used: its default, the project whose own config holds it, and the projects that make an exception. */
export type Scope = {
  kind: IntegrationKind;
  /** Every daemon id behind the row. */
  ids: string[];
  /** On by default: for a project's own item that stays in its project, on there. */
  on: boolean;
  /** The project whose own config holds it. */
  origin?: IntegrationProject | undefined;
  /** A project's own item kept in its project, as it is until the user uses it in every project. */
  stays: boolean;
  /** The projects whose exception turns it the other way from its default. */
  exceptions: IntegrationProject[];
};

export const kindOf = (item: IntegrationServer | IntegrationSkill): IntegrationKind => "description" in item ? "skill" : "server";

export function scopeOf(item: IntegrationServer | IntegrationSkill, snapshot: IntegrationSnapshot): Scope {
  const { catalog, switches } = snapshot;
  const kind = kindOf(item);
  const ids = ("ids" in item ? item.ids : undefined) ?? [item.id];
  const on = !(kind === "server" ? switches.mcpServersOff : switches.skillsOff).includes(item.id);
  const known = item.projectId ? catalog.projects?.find((project) => project.id === item.projectId) : undefined;
  const origin = known ?? (item.project ? { id: item.projectId ?? "", name: item.project.split("/").pop()!, path: item.projectPath ?? item.project, trusted: true } : undefined);
  const everywhere = (kind === "server" ? switches.serversEverywhere : switches.skillsEverywhere) ?? [];
  const stays = !!origin && !ids.some((id) => everywhere.includes(id));
  const exceptions = stays ? [] : (catalog.projects ?? []).filter((project) => {
    const entry = switches.projectSwitches?.find((item) => item.projectId === project.id);
    if (!entry) return false;
    const list = kind === "server" ? on ? entry.serversOff : entry.serversOn : on ? entry.skillsOff : entry.skillsOn;
    return ids.some((id) => list.includes(id));
  });
  return { kind, ids, on, origin, stays, exceptions };
}

/** Whether sessions in project `projectId` get it. */
export function usedIn(scope: Scope, projectId: string): boolean {
  if (scope.stays) return scope.origin?.id === projectId && scope.on;
  return scope.exceptions.some((project) => project.id === projectId) ? !scope.on : scope.on;
}

/** Whether sessions in any project get it. */
export const usedAnywhere = (scope: Scope) => scope.on || scope.exceptions.length > 0;
