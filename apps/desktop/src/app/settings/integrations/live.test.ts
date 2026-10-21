import assert from "node:assert/strict";
import { test } from "node:test";

import { mergeViews, snapshotOf } from "@/app/settings/integrations/live";
import type { IntegrationsView, McpServerView, Project, SkillView } from "@/ipc/generated";

const FILES: Record<string, string> = { "Claude Code (user)": "/Users/me/.claude.json", "Codex (user)": "/Users/me/.config/codex/config.toml" };
const railway = (source: string, id: string, off: boolean, nativeTo: McpServerView["nativeTo"]): McpServerView => ({
  id, name: "railway", source, project: null, plugin: null, file: FILES[source] ?? "", off, nativeTo, signIn: null,
  transport: { type: "stdio", command: "npx", args: ["-y", "@railway/mcp-server"], env: [], cwd: null },
});
const linear: McpServerView = {
  id: "claude:user:linear", name: "linear", source: "Claude Code (user)", project: null, plugin: null, file: "/Users/me/.claude.json", off: false, nativeTo: ["claude"],
  transport: { type: "http", url: "https://mcp.linear.app/mcp", headers: [] },
  signIn: { claude: { type: "native" }, codex: { type: "unknown" } },
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
  const view = mergeViews(user, [{ ...user, servers: [...user.servers, db] }]);
  view.servers.push(pluginServer);
  view.skills.push(pluginSkill);
  const { snapshot, members } = snapshotOf(view, [app, project("scratch", "/Users/me/scratch", false)], ["scratch"], 1);

  const [rail, lin, dbRow, calc, figma] = snapshot.catalog.servers;
  assert.equal(snapshot.catalog.servers.length, 5);
  assert.deepEqual(calc!.sources, ["CodexPlugin"]);
  assert.deepEqual(calc!.sourceFiles, [{ source: "CodexPlugin", path: "~/.codex/plugins/cache/openai-primary-runtime/spreadsheets/26.1017.22135/.mcp.json", plugin: "spreadsheets@openai-primary-runtime" }]);
  assert.deepEqual(rail!.sources, ["ClaudeUser", "CodexUser"]);
  assert.deepEqual(rail!.nativeAgents, ["claude", "codex"]);
  assert.deepEqual(rail!.sourceFiles, [{ source: "ClaudeUser", path: "~/.claude.json" }, { source: "CodexUser", path: "~/.config/codex/config.toml" }]);
  assert.deepEqual(members.get(rail!.id), ["claude:user:railway", "codex:user:railway"]);
  assert.deepEqual(snapshot.switches.mcpServersOff, ["claude:user:railway"]);
  assert.deepEqual(lin!.signIn, { claude: { kind: "native" }, codex: { kind: "required" } });
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
  const chart = snapshot.catalog.skills[1]!;
  assert.equal(chart.name, "spreadsheets:chart");
  assert.deepEqual(chart.locationDetails, [{ path: "~/.codex/plugins/cache/openai-primary-runtime/spreadsheets/26.1017.22135/skills/chart", plugin: "spreadsheets@openai-primary-runtime", agents: ["codex"] }]);
  assert.deepEqual(snapshot.catalog.untrustedProjects, ["scratch"]);
});
