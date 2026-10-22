import type { IntegrationSnapshot, IntegrationAdapter } from "@/app/settings/integrations/adapter";
import type {
  IntegrationAgent,
  IntegrationKind,
  IntegrationServer,
  IntegrationSignIn,
  IntegrationSkill,
  IntegrationSource,
} from "@/app/settings/integrations/types";
import type { IntegrationsView, McpServerView, ProblemView, Project, ProjectSwitches, ProviderKind, Settings, SignInState, SkillView } from "@/ipc/generated";
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
  "Claude Code (plugin)": "ClaudePlugin",
  "Codex (plugin)": "CodexPlugin",
};

function source(label: string): IntegrationSource {
  return SOURCES[label] ?? "ClaudeUser";
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

/** The project an entry belongs to, by name when it is one of the user's projects, with its id and folder. */
function projectOf(path: string | null, projects: Project[]): { project?: string; projectId?: string; projectPath?: string } {
  if (!path) return {};
  const project = projects.find((item) => item.repos[0]?.path === path);
  return { project: project?.name ?? shortPath(path), ...(project ? { projectId: project.id } : {}), projectPath: shortPath(path) };
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
    ids: members.map((member) => member.id),
    ...projectOf(first.project, projects),
    transport: transport.type === "stdio"
      ? { kind: "stdio", command: transport.command, args: transport.args, ...(transport.cwd ? { cwd: shortPath(transport.cwd) } : {}) }
      : { kind: "http", url: transport.url },
    env: transport.type === "stdio" ? transport.env : [],
    headers: transport.type === "http" ? transport.headers : [],
    signIn: states,
    ...(pending ? { pendingSignIn: pending } : {}),
    nativeAgents: agents,
    canSwitchOff: true,
    sourceFiles: members.map((member) => ({ source: source(member.source), path: shortPath(member.file), ...(member.plugin ? { plugin: member.plugin } : {}) })),
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

/** Whether a daemon id names an item of the project at `root`: `claude:project:<root>:<name>`, or a plugin's installed for it, `claude:plugin:<root>:<plugin>:…`. */
const ownedBy = (id: string, root: string) => id.includes(`:project:${root}:`) || id.includes(`:plugin:${root}:`);

function skill(view: SkillView, projects: Project[]): IntegrationSkill {
  // A user's skill linked into a project stays the user's: its id names its own root.
  const project = projects.find((item) => {
    const root = item.repos[0]?.path;
    return root && ownedBy(view.id, root);
  });
  const locations = view.locations.length ? view.locations : [{ path: view.path, linkedTo: null, plugin: null, nativeTo: view.nativeTo }];
  return {
    id: view.id,
    // A plugin's skill, as its agent shows it.
    name: view.plugin ? `${view.plugin}:${view.name}` : view.name,
    description: view.description,
    locations: locations.map((location) => shortPath(location.path)),
    locationDetails: locations.map((location) => ({
      path: shortPath(location.path),
      ...(location.linkedTo ? { linkedTo: shortPath(location.linkedTo) } : {}),
      ...(location.plugin ? { plugin: location.plugin } : {}),
      agents: location.nativeTo.map(agentOf),
    })),
    nativeAgents: view.nativeTo.map(agentOf),
    ...(project ? { project: project.name, projectId: project.id, projectPath: shortPath(project.repos[0]!.path) } : {}),
    // Claude switches its own skills off by name, so every skill can be switched off.
    canSwitchOff: true,
    claudeOffByName: view.claudeOffByName,
  };
}

function trusted(project: Project): boolean {
  const root = project.repos[0]?.path;
  return !!root && project.trust.some((folder) => folder.path === root && folder.trusted);
}

const known = (list: { id: string }[], id: string) => list.some((item) => item.id === id);

/**
 * The user's catalog plus each trusted project's own entries, by its folder. A project's
 * catalog also holds other projects' items used everywhere, read as that project sees them:
 * the list takes each item from its own project's.
 */
export function mergeViews(user: IntegrationsView, perProject: [string, IntegrationsView][]): IntegrationsView {
  const merged: IntegrationsView = { servers: [...user.servers], skills: [...user.skills], problems: [...user.problems] };
  for (const [root, view] of perProject) {
    merged.servers.push(...view.servers.filter((item) => item.project === root && !known(merged.servers, item.id)));
    merged.skills.push(...view.skills.filter((item) => ownedBy(item.id, root) && !known(merged.skills, item.id)));
    merged.problems.push(...view.problems.filter((item) => item.project === root && !known(merged.problems, item.id)));
  }
  return merged;
}

/** The daemon's server and project a sign-in request names. */
export type SignInTarget = { serverId: string; projectId: string | null };

/** The rows of one catalog, with the daemon ids behind each server row and what a sign-in to it names. */
function rowsOf(view: IntegrationsView, projects: Project[]) {
  const groups = groupServers(view.servers);
  return {
    groups,
    servers: [...groups.map((group) => server(group, projects)), ...view.problems.map((item) => problem(item, projects))],
    skills: view.skills.map((item) => skill(item, projects)),
  };
}

/** Each server id's copies across catalogs: groups that share a copy are one server's. */
function copiesOf(groups: McpServerView[][]): Map<string, string[]> {
  const copies = new Map<string, string[]>();
  for (const group of groups) {
    const ids = new Set(group.map((member) => member.id));
    for (const member of group) for (const other of copies.get(member.id) ?? []) ids.add(other);
    const all = [...ids];
    for (const id of all) copies.set(id, all);
  }
  return copies;
}

/** The page's snapshot of the daemon's `view` (and each trusted project's, by id), the daemon ids behind each server row and what a sign-in to it names. */
export function snapshotOf(view: IntegrationsView, projects: Project[], untrusted: string[], trustedCount: number, settings?: Settings, projectViews: Record<string, IntegrationsView> = {}) {
  const rows = rowsOf(view, projects);
  const perProject = Object.entries(projectViews).map(([id, item]) => [id, rowsOf(item, projects)] as const);
  const allGroups = [...rows.groups, ...perProject.flatMap(([, item]) => item.groups)];
  const members = copiesOf(allGroups);
  // A row's switch and exceptions cover its copies in every catalog, not only in its own.
  for (const row of [...rows.servers, ...perProject.flatMap(([, item]) => item.servers)]) {
    const copies = members.get(row.id);
    if (copies) row.ids = copies;
  }
  const targets = new Map<string, SignInTarget>(allGroups.map(([first]) => [first!.id, {
    serverId: first!.id,
    projectId: first!.project ? projects.find((project) => project.repos[0]?.path === first!.project)?.id ?? null : null,
  }]));
  const snapshot: IntegrationSnapshot = {
    catalog: {
      servers: rows.servers,
      skills: rows.skills,
      trustedProjectCount: trustedCount,
      untrustedProjects: untrusted,
      projects: projects.map((project) => ({ id: project.id, name: project.name, path: shortPath(project.repos[0]!.path), trusted: trusted(project) })),
      projectCatalogs: Object.fromEntries(perProject.map(([id, item]) => [id, { servers: item.servers, skills: item.skills }])),
    },
    switches: {
      mcpServersOff: rows.groups.filter((group) => group.every((member) => member.off)).map((group) => group[0]!.id),
      skillsOff: view.skills.filter((item) => item.off).map((item) => item.id),
      projectSwitches: settings?.projectSwitches ?? [],
      serversEverywhere: settings?.serversEverywhere ?? [],
      skillsEverywhere: settings?.skillsEverywhere ?? [],
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
  for (const item of view.servers) add(item.file);
  return paths;
}

function toggle(list: string[], ids: string[], enabled: boolean): string[] {
  return enabled ? list.filter((id) => !ids.includes(id)) : [...list, ...ids.filter((id) => !list.includes(id))];
}

type SwitchKeys = { on: "serversOn" | "skillsOn"; off: "serversOff" | "skillsOff"; global: "mcpServersOff" | "skillsOff"; everywhere: "serversEverywhere" | "skillsEverywhere" };
const KEYS: Record<IntegrationKind, SwitchKeys> = {
  server: { on: "serversOn", off: "serversOff", global: "mcpServersOff", everywhere: "serversEverywhere" },
  skill: { on: "skillsOn", off: "skillsOff", global: "skillsOff", everywhere: "skillsEverywhere" },
};
/** The settings an exception edit changes. */
export type SwitchSettings = Pick<Settings, "mcpServersOff" | "skillsOff" | "projectSwitches" | "serversEverywhere" | "skillsEverywhere">;
const emptySwitches = (entry: ProjectSwitches) => !entry.serversOn.length && !entry.serversOff.length && !entry.skillsOn.length && !entry.skillsOff.length;

/** `settings` with `edit` applied to project `projectId`'s exceptions, dropping an entry left empty. */
function editProject<S extends SwitchSettings>(settings: S, projectId: string, edit: (entry: ProjectSwitches) => ProjectSwitches): S {
  const found = settings.projectSwitches.some((entry) => entry.projectId === projectId);
  const entries = found ? settings.projectSwitches : [...settings.projectSwitches, { projectId, serversOn: [], serversOff: [], skillsOn: [], skillsOff: [] }];
  return { ...settings, projectSwitches: entries.map((entry) => entry.projectId === projectId ? edit(entry) : entry).filter((entry) => !emptySwitches(entry)) };
}

/**
 * Turns the item `ids` on or off in project `projectId`, or puts the project back on the
 * default (`null`): an exception is kept only where it differs from an id's default.
 */
export function inProject<S extends SwitchSettings>(settings: S, kind: IntegrationKind, ids: string[], projectId: string, enabled: boolean | null): S {
  const keys = KEYS[kind];
  return editProject(settings, projectId, (entry) => {
    const on = entry[keys.on].filter((id) => !ids.includes(id));
    const off = entry[keys.off].filter((id) => !ids.includes(id));
    for (const id of ids) {
      if (enabled === null) continue;
      const offByDefault = settings[keys.global].includes(id);
      if (enabled && offByDefault) on.push(id);
      if (!enabled && !offByDefault) off.push(id);
    }
    return { ...entry, [keys.on]: on, [keys.off]: off };
  });
}

/**
 * A project's own item used in every project (`on`), or kept in its project `origin` again.
 * Used everywhere it is on by default, and stays off in `origin` if it was; kept in `origin`
 * again, it keeps its on or off there and every project's exceptions to it go.
 */
export function everywhere<S extends SwitchSettings>(settings: S, kind: IntegrationKind, ids: string[], origin: string, on: boolean): S {
  const keys = KEYS[kind];
  if (on) {
    const offInOrigin = ids.filter((id) => settings[keys.global].includes(id));
    let next: S = { ...settings, [keys.global]: toggle(settings[keys.global], ids, true), [keys.everywhere]: toggle(settings[keys.everywhere], ids, false) };
    if (offInOrigin.length) next = inProject(next, kind, offInOrigin, origin, false);
    return next;
  }
  const entry = settings.projectSwitches.find((item) => item.projectId === origin);
  const offInOrigin = ids.filter((id) => settings[keys.global].includes(id) ? !entry?.[keys.on].includes(id) : !!entry?.[keys.off].includes(id));
  const projectSwitches = settings.projectSwitches
    .map((item) => ({ ...item, [keys.on]: item[keys.on].filter((id) => !ids.includes(id)), [keys.off]: item[keys.off].filter((id) => !ids.includes(id)) }))
    .filter((item) => !emptySwitches(item));
  const global = [...settings[keys.global].filter((id) => !ids.includes(id)), ...offInOrigin];
  return { ...settings, [keys.global]: global, [keys.everywhere]: settings[keys.everywhere].filter((id) => !ids.includes(id)), projectSwitches };
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
      const views = perProject.map((answer) => answer.integrations);
      const view = mergeViews(user.integrations, trustedProjects.map((project, index) => [project.repos[0]!.path, views[index]!]));
      const settings = useApp.getState().settings;
      // Each project's catalog as its sessions get it, with the items used everywhere.
      const projectViews = Object.fromEntries(trustedProjects.map((project, index) => [project.id, views[index]!]));
      const result = snapshotOf(view, projects, projects.filter((project) => !trusted(project)).map((project) => project.name), trustedProjects.length, settings, projectViews);
      members = result.members;
      targets = result.targets;
      paths = new Map([view, ...Object.values(projectViews)].flatMap((item) => [...pathsOf(item)]));
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
    setInProject: async (kind, ids, projectId, enabled) => {
      await editSettings((settings) => inProject(settings, kind, ids, projectId, enabled));
    },
    setEverywhere: async (kind, ids, origin, on) => {
      await editSettings((settings) => everywhere(settings, kind, ids, origin, on));
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
