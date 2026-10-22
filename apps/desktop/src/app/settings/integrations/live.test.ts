import assert from "node:assert/strict";
import { test } from "node:test";

import { everywhere, inProject, mergeViews, signIn, snapshotOf, type SwitchSettings } from "@/app/settings/integrations/live";
import type { IntegrationsView, McpServerView, Project, ProjectSwitches, SignInState, SkillView } from "@/ipc/generated";

const FILES: Record<string, string> = { "Claude Code (user)": "/Users/me/.claude.json", "Codex (user)": "/Users/me/.config/codex/config.toml" };
const railway = (source: string, id: string, off: boolean, nativeTo: McpServerView["nativeTo"]): McpServerView => ({
  id, name: "railway", source, project: null, plugin: null, file: FILES[source] ?? "", off, nativeTo, signIn: null, pendingSignIn: null, copyKey: "railway-key",
  transport: { type: "stdio", command: "npx", args: ["-y", "@railway/mcp-server"], env: [], cwd: null },
});
const linear: McpServerView = {
  id: "claude:user:linear", name: "linear", source: "Claude Code (user)", project: null, plugin: null, file: "/Users/me/.claude.json", off: false, nativeTo: ["claude"],
  transport: { type: "http", url: "https://mcp.linear.app/mcp", headers: [] },
  signIn: { claude: { type: "native" }, codex: { type: "unknown" } }, pendingSignIn: null, copyKey: "linear-key",
};
const shared: SkillView = {
  id: "agents:user:grill-me", name: "grill-me", description: "Interview you.", path: "/Users/me/.agents/skills/grill-me",
  nativeTo: ["claude", "codex"], off: true, claudeOffByName: true, plugin: null,
  locations: [
    { path: "/Users/me/.agents/skills/grill-me", linkedTo: null, plugin: null, nativeTo: ["codex"] },
    { path: "/Users/me/.claude/skills/grill-me", linkedTo: "/Users/me/.agents/skills/grill-me", plugin: null, nativeTo: ["claude"] },
  ],
};
const pluginRoot = "/Users/me/.codex/plugins/cache/openai-primary-runtime/spreadsheets/26.1017.22135";
const pluginSkill: SkillView = {
  id: "codex:plugin:spreadsheets@openai-primary-runtime:skills/chart", name: "chart", description: "Charts.", path: `${pluginRoot}/skills/chart`,
  nativeTo: ["codex"], off: false, claudeOffByName: false, plugin: "spreadsheets",
  locations: [{ path: `${pluginRoot}/skills/chart`, linkedTo: null, plugin: "spreadsheets@openai-primary-runtime", nativeTo: ["codex"] }],
};
const pluginServer: McpServerView = {
  ...railway("Codex (plugin)", "codex:plugin:spreadsheets@openai-primary-runtime:calc", false, ["codex"]),
  name: "calc", plugin: "spreadsheets@openai-primary-runtime", file: `${pluginRoot}/.mcp.json`,
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
  const db: McpServerView = { ...railway("Claude Code (project)", "claude:project:/Users/me/Development/brigadier-ai:db", false, ["claude"]), name: "db", project: "/Users/me/Development/brigadier-ai", file: "/Users/me/Development/brigadier-ai/.mcp.json" };
  const root = "/Users/me/Development/brigadier-ai";
  const projectPlugin: SkillView = {
    ...pluginSkill, id: `claude:plugin:${root}:docs@market:skills/write`, name: "write", plugin: "docs", path: "/Users/me/.claude/plugins/cache/market/docs/1.0.0/skills/write",
    locations: [], nativeTo: ["claude"],
  };
  const view = mergeViews(user, [[root, { ...user, servers: [...user.servers, db], skills: [...user.skills, projectPlugin] }]]);
  view.servers.push(pluginServer);
  view.skills.push(pluginSkill);
  const { snapshot, members, targets } = snapshotOf(view, [app, project("scratch", "/Users/me/scratch", false)], ["scratch"], 1);

  const [rail, lin, dbRow, calc, figma] = snapshot.catalog.servers;
  assert.equal(snapshot.catalog.servers.length, 5);
  assert.deepEqual(calc!.sources, ["CodexPlugin"]);
  assert.deepEqual(calc!.sourceFiles, [{ source: "CodexPlugin", path: "~/.codex/plugins/cache/openai-primary-runtime/spreadsheets/26.1017.22135/.mcp.json", plugin: "spreadsheets@openai-primary-runtime" }]);
  assert.deepEqual(rail!.sources, ["ClaudeUser", "CodexUser"]);
  assert.deepEqual(rail!.nativeAgents, ["claude", "codex"]);
  assert.deepEqual(rail!.sourceFiles, [{ source: "ClaudeUser", path: "~/.claude.json" }, { source: "CodexUser", path: "~/.config/codex/config.toml" }]);
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
  const chart = snapshot.catalog.skills.find((item) => item.name === "spreadsheets:chart")!;
  assert.equal(chart.name, "spreadsheets:chart");
  assert.deepEqual(chart.locationDetails, [{ path: "~/.codex/plugins/cache/openai-primary-runtime/spreadsheets/26.1017.22135/skills/chart", plugin: "spreadsheets@openai-primary-runtime", agents: ["codex"] }]);
  // A plugin installed for the project is the project's.
  const write = snapshot.catalog.skills.find((item) => item.name === "docs:write")!;
  assert.equal(write.projectId, "brigadier-ai");
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

test("project view: each project's catalog as the daemon reads it, and the list takes each item from its own project's", () => {
  const app = project("app", "/Users/me/app", true);
  const webProject = project("web", "/Users/me/web", true);
  const user: IntegrationsView = { servers: [linear], skills: [shared], problems: [] };
  const api: McpServerView = { ...railway("Claude Code (project)", "claude:project:/Users/me/web:api", false, ["claude"]), name: "api", project: "/Users/me/web", copyKey: "api-key", transport: { type: "stdio", command: "node", args: ["index.js"], env: [], cwd: "/Users/me/web/tools/api" } };
  // In app, its .mcp.json's linear overrides the user's; web's api is used everywhere, which Claude doesn't load in app.
  const appLinear: McpServerView = { ...linear, id: "claude:project:/Users/me/app:linear", source: "Claude Code (project)", project: "/Users/me/app", transport: { type: "http", url: "https://linear.example/mcp", headers: [] } };
  const appOwn: IntegrationsView = { servers: [appLinear, { ...api, nativeTo: [] }], skills: [shared], problems: [] };
  const webOwn: IntegrationsView = { servers: [linear, api], skills: [shared], problems: [] };
  const merged = mergeViews(user, [["/Users/me/app", appOwn], ["/Users/me/web", webOwn]]);
  assert.deepEqual(merged.servers.map((item) => [item.id, item.nativeTo]), [[linear.id, ["claude"]], [appLinear.id, ["claude"]], [api.id, ["claude"]]]);
  const { snapshot } = snapshotOf(merged, [app, webProject], [], 2, undefined, { app: appOwn });
  const view = snapshot.catalog.projectCatalogs!.app!;
  assert.deepEqual(view.servers.map((item) => [item.name, item.project, item.nativeAgents, item.transport]), [
    ["linear", "app", ["claude"], { kind: "http", url: "https://linear.example/mcp" }],
    ["api", "web", [], { kind: "stdio", command: "node", args: ["index.js"], cwd: "~/web/tools/api" }],
  ]);
});

/** A copy of web's api server. */
const copy = (id: string, source: string, nativeTo: McpServerView["nativeTo"]): McpServerView => ({ ...railway(source, id, false, nativeTo), name: "api", project: "/Users/me/web", copyKey: "api-key" });

test("a server's row covers its copies in every catalog, so its switch turns every copy off", () => {
  const app = project("app", "/Users/me/app", true);
  const webProject = project("web", "/Users/me/web", true);
  const check = (user: IntegrationsView, web: IntegrationsView, inApp: IntegrationsView, all: string[]) => {
    const { snapshot, members } = snapshotOf(mergeViews(user, [["/Users/me/app", inApp], ["/Users/me/web", web]]), [app, webProject], [], 2, undefined, { app: inApp, web });
    const rows = [...snapshot.catalog.servers, ...Object.values(snapshot.catalog.projectCatalogs!).flatMap((item) => item.servers)].filter((row) => row.name === "api");
    for (const row of rows) {
      assert.deepEqual(row.ids!.toSorted(), all.toSorted());
      assert.deepEqual(members.get(row.id)!.toSorted(), all.toSorted());
    }
  };
  // Used everywhere, then copied into Codex's config in web: app's catalog has only the first copy.
  const claude = copy("claude:project:/Users/me/web:api", "Claude Code (project)", ["claude"]);
  const codex = copy("codex:project:/Users/me/web:api", "Codex (project)", ["codex"]);
  const empty: IntegrationsView = { servers: [], skills: [], problems: [] };
  check(empty, { ...empty, servers: [claude, codex] }, { ...empty, servers: [{ ...claude, nativeTo: [] }] }, [claude.id, codex.id]);
  // A Claude local-scope copy and a .mcp.json copy are one row; another project's catalog has only the local one.
  const local = copy("claude:local:/Users/me/web:api", "Claude Code (local)", ["claude"]);
  check({ ...empty, servers: [local] }, { ...empty, servers: [local, claude] }, { ...empty, servers: [{ ...local, nativeTo: [] }] }, [local.id, claude.id]);
});
