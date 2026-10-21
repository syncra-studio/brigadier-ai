import assert from "node:assert/strict";
import { test } from "node:test";

import { everywhere, inProject, mergeViews, projectView, signIn, snapshotOf, type SwitchSettings } from "@/app/settings/integrations/live";
import type { IntegrationsView, McpServerView, Project, ProjectSwitches, Settings, SignInState, SkillView } from "@/ipc/generated";

const railway = (source: string, id: string, off: boolean, nativeTo: McpServerView["nativeTo"]): McpServerView => ({
  id, name: "railway", source, project: null, off, nativeTo, signIn: null, pendingSignIn: null, copyKey: "railway-key",
  transport: { type: "stdio", command: "npx", args: ["-y", "@railway/mcp-server"], env: [], cwd: null },
});
const linear: McpServerView = {
  id: "claude:user:linear", name: "linear", source: "Claude Code (user)", project: null, off: false, nativeTo: ["claude"],
  transport: { type: "http", url: "https://mcp.linear.app/mcp", headers: [] },
  signIn: { claude: { type: "native" }, codex: { type: "unknown" } }, pendingSignIn: null, copyKey: "linear-key",
};
const shared: SkillView = {
  id: "agents:user:grill-me", name: "grill-me", description: "Interview you.", path: "/Users/me/.agents/skills/grill-me",
  nativeTo: ["claude", "codex"], off: true, claudeOffByName: true,
  locations: [
    { path: "/Users/me/.agents/skills/grill-me", linkedTo: null, nativeTo: ["codex"] },
    { path: "/Users/me/.claude/skills/grill-me", linkedTo: "/Users/me/.agents/skills/grill-me", nativeTo: ["claude"] },
  ],
};
const project = (name: string, path: string, trusted: boolean): Project => ({
  id: name, name, createdAtMs: 0, repos: [{ path, name }], prefs: {} as Project["prefs"],
  trust: [{ path, trusted, decidedAtMs: 0 }],
});

test("live catalog: one row per server found in several configs, its switch covering every copy", () => {
  const user: IntegrationsView = {
    servers: [railway("Claude Code (user)", "claude:user:railway", true, ["claude"]), railway("Codex (user)", "codex:user:railway", true, ["codex"]), linear],
    skills: [shared],
    problems: [{ id: "cursor:user:figma", name: "figma", source: "Cursor (user)", project: null, file: "/Users/me/.cursor/mcp.json", line: null, read: "ws://127.0.0.1:3845/mcp", error: "Unsupported address “ws://”: only http and https" }],
  };
  const app = project("brigadier-ai", "/Users/me/Development/brigadier-ai", true);
  const db: McpServerView = { ...railway("Claude Code (project)", "claude:project:/Users/me/Development/brigadier-ai:db", false, ["claude"]), name: "db", project: "/Users/me/Development/brigadier-ai" };
  const view = mergeViews(user, [{ ...user, servers: [...user.servers, db] }]);
  const { snapshot, members, targets } = snapshotOf(view, [app, project("scratch", "/Users/me/scratch", false)], ["scratch"], 1);

  const [rail, lin, dbRow, figma] = snapshot.catalog.servers;
  assert.equal(snapshot.catalog.servers.length, 4);
  assert.deepEqual(rail!.sources, ["ClaudeUser", "CodexUser"]);
  assert.deepEqual(rail!.nativeAgents, ["claude", "codex"]);
  assert.deepEqual(members.get(rail!.id), ["claude:user:railway", "codex:user:railway"]);
  assert.deepEqual(snapshot.switches.mcpServersOff, ["claude:user:railway"]);
  assert.deepEqual(lin!.signIn, { claude: { kind: "native" }, codex: { kind: "unknown" } });
  assert.equal(lin!.pendingSignIn, undefined);
  assert.equal(rail!.pendingSignIn, undefined);
  // A sign-in names the daemon's first copy of a server, and its project for a project's own.
  assert.deepEqual(targets.get(rail!.id), { serverId: "claude:user:railway", projectId: null });
  assert.deepEqual(targets.get(dbRow!.id), { serverId: "claude:project:/Users/me/Development/brigadier-ai:db", projectId: "brigadier-ai" });
  // Two copies with one name and command but other environment values are two servers.
  const otherKey = { ...railway("Codex (user)", "codex:user:railway", false, ["codex"]), copyKey: "other-railway-key" };
  const split = snapshotOf(mergeViews({ ...user, servers: [user.servers[0]!, otherKey], problems: [] }, []), [], [], 1).snapshot.catalog.servers;
  assert.equal(split.length, 2);
  assert.equal(dbRow!.project, "brigadier-ai");
  assert.deepEqual(dbRow!.sourceFiles, [{ source: "ClaudeProjectFile", path: "~/Development/brigadier-ai/.mcp.json" }]);
  assert.equal(figma!.error, "Unsupported address “ws://”: only http and https");
  assert.deepEqual(figma!.transport, { kind: "http", url: "ws://127.0.0.1:3845/mcp" });
  assert.equal(figma!.canSwitchOff, false);

  const [skill] = snapshot.catalog.skills;
  assert.deepEqual(skill!.locationDetails, [
    { path: "~/.agents/skills/grill-me", agents: ["codex"] },
    { path: "~/.claude/skills/grill-me", linkedTo: "~/.agents/skills/grill-me", agents: ["claude"] },
  ]);
  assert.deepEqual(snapshot.switches.skillsOff, ["agents:user:grill-me"]);
  assert.deepEqual(snapshot.catalog.untrustedProjects, ["scratch"]);
});

/** Two copies of linear with these agent states. */
const copies = (first: [SignInState, SignInState], second: [SignInState, SignInState], pending: string | null = null) => snapshotOf({
  servers: [
    { ...linear, signIn: { claude: first[0], codex: first[1] } },
    { ...linear, id: "codex:user:linear", source: "Codex (user)", nativeTo: ["codex"], signIn: { claude: second[0], codex: second[1] }, pendingSignIn: pending },
  ],
  skills: [],
  problems: [],
}, [], [], 0).snapshot.catalog.servers;

test("live catalog: each agent's sign-in state, the best one across copies and a sign-in in progress", () => {
  assert.deepEqual(signIn({ type: "native" }), { kind: "native" });
  assert.deepEqual(signIn({ type: "reusableFrom", agent: "claude" }), { kind: "reusable", from: "claude" });
  assert.deepEqual(signIn({ type: "inBrigadier", sinceMs: 1792584000000 }), { kind: "brigadier", sinceMs: 1792584000000 });
  assert.deepEqual(signIn({ type: "inBrigadier", sinceMs: null }), { kind: "brigadier" });
  assert.deepEqual(signIn({ type: "needed" }), { kind: "needed" });
  assert.deepEqual(signIn({ type: "unknown" }), { kind: "unknown" });
  assert.deepEqual(signIn({ type: "notAllowed", agent: "codex" }), { kind: "notAllowed", from: "codex" });
  assert.deepEqual(signIn({ type: "unsupported" }), { kind: "unsupported" });

  // A copy's own sign-in wins, then a reused one, then Brigadier's; otherwise the first copy's state.
  const [best] = copies([{ type: "needed" }, { type: "inBrigadier", sinceMs: null }], [{ type: "reusableFrom", agent: "codex" }, { type: "native" }]);
  assert.deepEqual(best!.signIn, { claude: { kind: "reusable", from: "codex" }, codex: { kind: "native" } });
  const [neither] = copies([{ type: "unknown" }, { type: "needed" }], [{ type: "needed" }, { type: "notAllowed", agent: "claude" }], "sign-in-7");
  assert.deepEqual(neither!.signIn, { claude: { kind: "unknown" }, codex: { kind: "needed" } });
  assert.equal(neither!.pendingSignIn, "sign-in-7");
  // A local server has no sign-in.
  assert.deepEqual(snapshotOf({ servers: [railway("Claude Code (user)", "claude:user:railway", false, ["claude"])], skills: [], problems: [] }, [], [], 0).snapshot.catalog.servers[0]!.signIn, {});
});

const switches = (over: Partial<SwitchSettings> = {}): SwitchSettings => ({ mcpServersOff: [], skillsOff: [], projectSwitches: [], serversEverywhere: [], skillsEverywhere: [], ...over });
const entry = (projectId: string, over: Partial<ProjectSwitches>): ProjectSwitches => ({ projectId, serversOn: [], serversOff: [], skillsOn: [], skillsOff: [], ...over });
const RAILWAY = ["claude:user:railway", "codex:user:railway"];

test("project exceptions: one is kept only where it differs from the default, for all of a row's ids, and an emptied entry goes", () => {
  const web = entry("web", { skillsOff: ["agents:user:grill-me"] });
  // On by default: off in app adds every copy's id to app's off list; other projects stay as they are.
  const offInApp = inProject(switches({ projectSwitches: [web] }), "server", RAILWAY, "app", false);
  assert.deepEqual(offInApp.projectSwitches, [web, entry("app", { serversOff: RAILWAY })]);
  // Back on in app, or the exception removed, drops the ids and then the empty entry.
  assert.deepEqual(inProject(offInApp, "server", RAILWAY, "app", true).projectSwitches, [web]);
  assert.deepEqual(inProject(offInApp, "server", RAILWAY, "app", null).projectSwitches, [web]);
  // Off by default: on in app goes to its on list, and off again is the default, so nothing is kept.
  const offByDefault = switches({ mcpServersOff: RAILWAY });
  const onInApp = inProject(offByDefault, "server", RAILWAY, "app", true);
  assert.deepEqual(onInApp, { ...offByDefault, projectSwitches: [entry("app", { serversOn: RAILWAY })] });
  assert.deepEqual(inProject(onInApp, "server", RAILWAY, "app", false).projectSwitches, []);
  // An id is never in both lists, and an entry with other exceptions stays.
  const both = switches({ projectSwitches: [entry("app", { serversOn: [RAILWAY[0]!], skillsOn: ["agents:user:grill-me"] })] });
  assert.deepEqual(inProject(both, "server", RAILWAY, "app", false).projectSwitches, [entry("app", { serversOff: RAILWAY, skillsOn: ["agents:user:grill-me"] })]);
  // Skills use the skill lists.
  assert.deepEqual(inProject(switches(), "skill", ["agents:user:grill-me"], "app", false).projectSwitches, [entry("app", { skillsOff: ["agents:user:grill-me"] })]);
  // The defaults themselves never change.
  assert.deepEqual(inProject(offByDefault, "server", RAILWAY, "web", true).mcpServersOff, RAILWAY);
});

test("used in every project: on by default everywhere, and back in its project its exceptions go", () => {
  const DB = ["claude:project:/Users/me/app:db", "codex:project:/Users/me/app:db"];
  const other = entry("web", { serversOff: ["claude:user:railway"] });
  // On in its own project: used everywhere it's on by default, with no exception.
  const start = switches({ mcpServersOff: ["desktop:user:playwright"], projectSwitches: [other] });
  const used = everywhere(start, "server", DB, "app", true);
  assert.deepEqual(used, { ...start, serversEverywhere: DB });
  // An exception made since goes when it's kept in its project again, and so does the entry it emptied.
  const offInDocs = inProject(used, "server", DB, "docs", false);
  assert.deepEqual(offInDocs.projectSwitches, [other, entry("docs", { serversOff: DB })]);
  assert.deepEqual(everywhere(offInDocs, "server", DB, "app", false), start);
  // Off in its own project: used everywhere, it stays off there as app's exception; other projects' defaults don't change.
  const offStart = switches({ mcpServersOff: ["desktop:user:playwright", ...DB], projectSwitches: [other] });
  const offUsed = everywhere(offStart, "server", DB, "app", true);
  assert.deepEqual(offUsed, { ...offStart, mcpServersOff: ["desktop:user:playwright"], serversEverywhere: DB, projectSwitches: [other, entry("app", { serversOff: DB })] });
  // Kept in its project again, it's still off there, through the default list.
  assert.deepEqual(everywhere(offUsed, "server", DB, "app", false), offStart);
  // Turned on in its project while used everywhere, it comes back on there.
  assert.deepEqual(everywhere(inProject(offUsed, "server", DB, "app", null), "server", DB, "app", false), { ...offStart, mcpServersOff: ["desktop:user:playwright"] });
  // Skills use the skill lists.
  const skill = "claude:project:/Users/me/app:release-notes";
  assert.deepEqual(everywhere(switches({ skillsOff: [skill] }), "skill", [skill], "app", true), switches({ skillsEverywhere: [skill], projectSwitches: [entry("app", { skillsOff: [skill] })] }));
});

test("project view: a project's own catalog, with other projects' items used everywhere, and where a server runs", () => {
  const app = project("app", "/Users/me/app", true);
  const webProject = project("web", "/Users/me/web", true);
  const user: IntegrationsView = { servers: [linear], skills: [shared], problems: [] };
  // In app, its .mcp.json's linear overrides the user's, and the user's grill-me is linked into its .claude/skills.
  const appLinear: McpServerView = { ...linear, id: "claude:project:/Users/me/app:linear", source: "Claude Code (project)", project: "/Users/me/app", transport: { type: "http", url: "https://linear.example/mcp", headers: [] } };
  const appSkill: SkillView = { ...shared, locations: [...shared.locations, { path: "/Users/me/app/.claude/skills/grill-me", linkedTo: "/Users/me/.agents/skills/grill-me", nativeTo: ["claude"] }] };
  const appOwn: IntegrationsView = { servers: [appLinear], skills: [appSkill], problems: [] };
  const api: McpServerView = { ...railway("Claude Code (project)", "claude:project:/Users/me/web:api", false, ["claude"]), name: "api", project: "/Users/me/web", copyKey: "api-key", transport: { type: "stdio", command: "node", args: ["index.js"], env: [], cwd: "/Users/me/web/tools/api" } };
  const db: McpServerView = { ...api, id: "claude:project:/Users/me/web:db", name: "db", copyKey: "db-key" };
  const webOwn: IntegrationsView = { servers: [linear, api, db], skills: [shared], problems: [] };
  const settings = { serversEverywhere: [api.id], skillsEverywhere: [] };

  const inApp = projectView(appOwn, "/Users/me/app", new Map([["/Users/me/app", appOwn], ["/Users/me/web", webOwn]]), settings);
  assert.deepEqual(inApp.servers.map((item) => item.id), [appLinear.id, api.id]);
  const merged = mergeViews(user, [appOwn, webOwn]);
  const { snapshot, members } = snapshotOf(merged, [app, webProject], [], 2, { ...settings, projectSwitches: [] } as unknown as Settings, { app: inApp });
  const view = snapshot.catalog.projectCatalogs!.app!;
  // The project's own version, not the merged catalog's first one.
  assert.deepEqual(view.servers.map((item) => [item.name, item.project, item.transport]), [
    ["linear", "app", { kind: "http", url: "https://linear.example/mcp" }],
    ["api", "web", { kind: "stdio", command: "node", args: ["index.js"], cwd: "~/web/tools/api" }],
  ]);
  assert.deepEqual(view.skills[0]!.locations, ["~/.agents/skills/grill-me", "~/.claude/skills/grill-me", "~/app/.claude/skills/grill-me"]);
  assert.deepEqual(snapshot.catalog.skills[0]!.locations, ["~/.agents/skills/grill-me", "~/.claude/skills/grill-me"]);
  assert.deepEqual(members.get(appLinear.id), [appLinear.id]);
  assert.equal(snapshot.catalog.servers.find((item) => item.id === linear.id)!.transport.kind, "http");
});
