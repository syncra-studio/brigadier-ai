import type { IntegrationSnapshot, IntegrationAdapter } from "@/app/settings/integrations/adapter";
import type {
  IntegrationAgent,
  IntegrationServer,
  IntegrationSignIn,
  IntegrationSkill,
  IntegrationSource,
} from "@/app/settings/integrations/types";
import type { IntegrationsView, McpServerView, ProblemView, Project, ProviderKind, SignInState, SkillView } from "@/ipc/generated";
import { request, revealPath } from "@/ipc/client";
import { shortPath } from "@/lib/format";
import { editSettings } from "@/state/settings";
import { useApp } from "@/state/store";

/*
 * The daemon's catalog for the Plugins page. The daemon lists a server once per config that
 * has it; the page shows one server found in several, so its switch covers every copy. A
 * project's servers and skills are read only for projects whose folder the user trusts.
 */

/** The configs keep changing while Settings is open; the catalog is cheap to read again. */
const POLL_MS = 5000;

const SOURCES: Record<string, IntegrationSource> = {
  "Claude Code (user)": "ClaudeUser",
  "Claude Code (local)": "ClaudeLocal",
  "Claude Code (project)": "ClaudeProjectFile",
  "Codex (user)": "CodexUser",
  "Codex (project)": "CodexProject",
  "Claude Desktop": "Desktop",
  "Cursor (user)": "CursorUser",
  "Cursor (project)": "CursorProject",
};

function source(label: string): IntegrationSource {
  return SOURCES[label] ?? "ClaudeUser";
}

/** The config file a source reads, with the project's real folder. */
function sourceFile(kind: IntegrationSource, project: string | null): string {
  const root = project ? shortPath(project) : "";
  switch (kind) {
    case "ClaudeUser":
    case "ClaudeLocal": return "~/.claude.json";
    case "ClaudeProjectFile": return `${root}/.mcp.json`;
    case "CodexUser": return "~/.codex/config.toml";
    case "CodexProject": return `${root}/.codex/config.toml`;
    case "Desktop": return "~/Library/Application Support/Claude/claude_desktop_config.json";
    case "CursorUser": return "~/.cursor/mcp.json";
    case "CursorProject": return `${root}/.cursor/mcp.json`;
  }
}

const agentOf = (provider: ProviderKind): IntegrationAgent => provider;

/** How an agent's workers are signed in, as the page shows it. */
export function signIn(state: SignInState): IntegrationSignIn {
  switch (state.type) {
    case "native": return { kind: "native" };
    case "reusableFrom": return { kind: "reusable", from: agentOf(state.agent) };
    case "inBrigadier": return state.sinceMs === null ? { kind: "brigadier" } : { kind: "brigadier", sinceMs: state.sinceMs };
    case "needed": return { kind: "needed" };
    case "unknown": return { kind: "unknown" };
    case "notAllowed": return { kind: "notAllowed", from: agentOf(state.agent) };
    case "unsupported": return { kind: "unsupported" };
  }
}

/** A copy's own sign-in first, then one reused or made in Brigadier. */
const SIGN_IN_ORDER: IntegrationSignIn["kind"][] = ["native", "reusable", "brigadier"];

/** Copies of one server: the same name and project, and a transport with the same values (`copyKey` hashes them). */
function sameServer(a: McpServerView, b: McpServerView): boolean {
  return a.name === b.name && a.project === b.project && a.copyKey === b.copyKey;
}

/** Each group of copies of one server, keyed by its first copy's id. */
export function groupServers(views: McpServerView[]): McpServerView[][] {
  const groups: McpServerView[][] = [];
  for (const view of views) {
    const group = groups.find((members) => sameServer(members[0]!, view));
    if (group) group.push(view);
    else groups.push([view]);
  }
  return groups;
}

/** The project an entry belongs to, by name when it is one of the user's projects. */
function projectOf(path: string | null, projects: Project[]): { project?: string } {
  if (!path) return {};
  return { project: projects.find((project) => project.repos[0]?.path === path)?.name ?? shortPath(path) };
}

function server(members: McpServerView[], projects: Project[]): IntegrationServer {
  const first = members[0]!;
  const agents = [...new Set(members.flatMap((member) => member.nativeTo.map(agentOf)))];
  const states: Partial<Record<IntegrationAgent, IntegrationSignIn>> = {};
  for (const agent of ["claude", "codex"] as const) {
    const all = members.flatMap((member) => (member.signIn ? [signIn(member.signIn[agent])] : []));
    const best = SIGN_IN_ORDER.map((kind) => all.find((state) => state.kind === kind)).find(Boolean) ?? all[0];
    if (best) states[agent] = best;
  }
  const transport = first.transport;
  const pending = members.find((member) => member.pendingSignIn)?.pendingSignIn;
  return {
    id: first.id,
    name: first.name,
    sources: members.map((member) => source(member.source)),
    ...projectOf(first.project, projects),
    transport: transport.type === "stdio" ? { kind: "stdio", command: transport.command, args: transport.args } : { kind: "http", url: transport.url },
    env: transport.type === "stdio" ? transport.env : [],
    headers: transport.type === "http" ? transport.headers : [],
    signIn: states,
    ...(pending ? { pendingSignIn: pending } : {}),
    nativeAgents: agents,
    canSwitchOff: true,
    sourceFiles: members.map((member) => ({ source: source(member.source), path: sourceFile(source(member.source), member.project) })),
  };
}

function problem(view: ProblemView, projects: Project[]): IntegrationServer {
  const kind = source(view.source);
  const read = view.read ?? shortPath(view.file);
  return {
    id: view.id,
    name: view.name,
    sources: [kind],
    ...projectOf(view.project, projects),
    transport: read.includes("://") ? { kind: "http", url: read } : { kind: "stdio", command: read, args: [] },
    env: [],
    headers: [],
    signIn: {},
    nativeAgents: [],
    canSwitchOff: false,
    error: view.error,
    sourceFiles: [{ source: kind, path: shortPath(view.file), ...(view.line ? { line: view.line } : {}) }],
  };
}

function skill(view: SkillView, projects: Project[]): IntegrationSkill {
  const project = projects.find((item) => {
    const root = item.repos[0]?.path;
    return root && view.locations.some((location) => location.path.startsWith(`${root}/`));
  });
  const locations = view.locations.length ? view.locations : [{ path: view.path, linkedTo: null, nativeTo: view.nativeTo }];
  return {
    id: view.id,
    name: view.name,
    description: view.description,
    locations: locations.map((location) => shortPath(location.path)),
    locationDetails: locations.map((location) => ({
      path: shortPath(location.path),
      ...(location.linkedTo ? { linkedTo: shortPath(location.linkedTo) } : {}),
      agents: location.nativeTo.map(agentOf),
    })),
    nativeAgents: view.nativeTo.map(agentOf),
    ...(project ? { project: project.name } : {}),
    // Claude switches its own skills off by name, so every skill can be switched off.
    canSwitchOff: true,
  };
}

function trusted(project: Project): boolean {
  const root = project.repos[0]?.path;
  return !!root && project.trust.some((folder) => folder.path === root && folder.trusted);
}

const known = (list: { id: string }[], id: string) => list.some((item) => item.id === id);

/** The user's catalog plus each trusted project's own entries. */
export function mergeViews(user: IntegrationsView, perProject: IntegrationsView[]): IntegrationsView {
  const merged: IntegrationsView = { servers: [...user.servers], skills: [...user.skills], problems: [...user.problems] };
  for (const view of perProject) {
    merged.servers.push(...view.servers.filter((item) => !known(merged.servers, item.id)));
    merged.skills.push(...view.skills.filter((item) => !known(merged.skills, item.id)));
    merged.problems.push(...view.problems.filter((item) => !known(merged.problems, item.id)));
  }
  return merged;
}

/** The daemon's server and project a sign-in request names. */
export type SignInTarget = { serverId: string; projectId: string | null };

/** The page's snapshot of the daemon's `view`, the daemon ids behind each server row and what a sign-in to it names. */
export function snapshotOf(view: IntegrationsView, projects: Project[], untrusted: string[], trustedCount: number) {
  const groups = groupServers(view.servers);
  const members = new Map(groups.map((group) => [group[0]!.id, group.map((member) => member.id)]));
  const targets = new Map<string, SignInTarget>(groups.map(([first]) => [first!.id, {
    serverId: first!.id,
    projectId: first!.project ? projects.find((project) => project.repos[0]?.path === first!.project)?.id ?? null : null,
  }]));
  const snapshot: IntegrationSnapshot = {
    catalog: {
      servers: [...groups.map((group) => server(group, projects)), ...view.problems.map((item) => problem(item, projects))],
      skills: view.skills.map((item) => skill(item, projects)),
      trustedProjectCount: trustedCount,
      untrustedProjects: untrusted,
    },
    switches: {
      mcpServersOff: groups.filter((group) => group.every((member) => member.off)).map((group) => group[0]!.id),
      skillsOff: view.skills.filter((item) => item.off).map((item) => item.id),
    },
  };
  return { snapshot, members, targets };
}

/** Each path shown, by its display form, for Show in Finder. */
function pathsOf(view: IntegrationsView): Map<string, string> {
  const paths = new Map<string, string>();
  const add = (path: string) => paths.set(shortPath(path), path);
  for (const item of view.skills) {
    add(item.path);
    for (const location of item.locations) add(location.path);
  }
  for (const item of view.problems) add(item.file);
  return paths;
}

function toggle(list: string[], ids: string[], enabled: boolean): string[] {
  return enabled ? list.filter((id) => !ids.includes(id)) : [...list, ...ids.filter((id) => !list.includes(id))];
}

/** The Plugins page's connection to the daemon. */
export function liveIntegrations(): IntegrationAdapter {
  let members = new Map<string, string[]>();
  let targets = new Map<string, SignInTarget>();
  let paths = new Map<string, string>();
  const target = (id: string): SignInTarget => targets.get(id) ?? { serverId: id, projectId: null };
  return {
    read: async () => {
      const projects = Object.values(useApp.getState().projects).filter((project) => project.repos.length > 0);
      const trustedProjects = projects.filter(trusted);
      const [user, ...perProject] = await Promise.all([
        request({ method: "getIntegrations", projectId: null }),
        ...trustedProjects.map((project) => request({ method: "getIntegrations", projectId: project.id })),
      ]);
      const view = mergeViews(user.integrations, perProject.map((answer) => answer.integrations));
      const result = snapshotOf(view, projects, projects.filter((project) => !trusted(project)).map((project) => project.name), trustedProjects.length);
      members = result.members;
      targets = result.targets;
      paths = pathsOf(view);
      return result.snapshot;
    },
    subscribe: (onChange) => {
      const timer = setInterval(onChange, POLL_MS);
      const unsubscribe = useApp.subscribe((state, previous) => {
        if (state.settings !== previous.settings || state.projects !== previous.projects) onChange();
      });
      return () => {
        clearInterval(timer);
        unsubscribe();
      };
    },
    setServerEnabled: async (id, enabled) => {
      const ids = members.get(id) ?? [id];
      await editSettings((settings) => ({ ...settings, mcpServersOff: toggle(settings.mcpServersOff, ids, enabled) }));
    },
    setSkillEnabled: async (id, enabled) => {
      await editSettings((settings) => ({ ...settings, skillsOff: toggle(settings.skillsOff, [id], enabled) }));
    },
    showPath: (path) => revealPath(paths.get(path) ?? path.replace(/^~(?=\/)/, homeOf(paths))),
    signIn: async (id) => (await request({ method: "signInMcpServer", ...target(id) })).id,
    signInStatus: async (signInId) => (await request({ method: "getMcpSignIn", id: signInId })).status,
    cancelSignIn: async (signInId) => { await request({ method: "cancelMcpSignIn", id: signInId }); },
    reopenSignIn: async (signInId) => { await request({ method: "reopenMcpSignIn", id: signInId }); },
    signOut: async (id) => { await request({ method: "signOutMcpServer", ...target(id) }); },
    askSignInAgain: async (id) => { await request({ method: "askMcpSignInAgain", ...target(id) }); },
  };
}

/** The home folder, from any absolute path the catalog named under it. */
function homeOf(paths: Map<string, string>): string {
  for (const [shown, full] of paths) {
    if (shown.startsWith("~/")) return full.slice(0, full.length - shown.length + 1);
  }
  return "~";
}
