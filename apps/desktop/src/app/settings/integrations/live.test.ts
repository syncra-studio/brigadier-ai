import assert from "node:assert/strict";
import { test } from "node:test";

import { mergeViews, snapshotOf } from "@/app/settings/integrations/live";
import type { IntegrationsView, McpServerView, Project, SignInState, SkillView } from "@/ipc/generated";

const railway = (source: string, id: string, off: boolean, nativeTo: McpServerView["nativeTo"]): McpServerView => ({
  id, name: "railway", source, project: null, off, nativeTo, signIn: null,
  transport: { type: "stdio", command: "npx", args: ["-y", "@railway/mcp-server"], env: [], cwd: null },
});
const linear: McpServerView = {
  id: "claude:user:linear", name: "linear", source: "Claude Code (user)", project: null, off: false, nativeTo: ["claude"],
  transport: { type: "http", url: "https://mcp.linear.app/mcp", headers: [] },
  signIn: { claude: { type: "native" }, codex: { type: "unknown" } },
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
  const { snapshot, members } = snapshotOf(view, [app, project("scratch", "/Users/me/scratch", false)], ["scratch"], 1);

  const [rail, lin, dbRow, figma] = snapshot.catalog.servers;
  assert.equal(snapshot.catalog.servers.length, 4);
  assert.deepEqual(rail!.sources, ["ClaudeUser", "CodexUser"]);
  assert.deepEqual(rail!.nativeAgents, ["claude", "codex"]);
  assert.deepEqual(members.get(rail!.id), ["claude:user:railway", "codex:user:railway"]);
  assert.deepEqual(snapshot.switches.mcpServersOff, ["claude:user:railway"]);
  assert.deepEqual(lin!.signIn, { claude: { kind: "native" }, codex: { kind: "required" } });
  const signIns = (claude: SignInState, codex: SignInState) =>
    snapshotOf(mergeViews({ ...user, servers: [{ ...linear, signIn: { claude, codex } }], problems: [] }, []), [], [], 1).snapshot.catalog.servers[0]!.signIn;
  assert.deepEqual(signIns({ type: "inBrigadier" }, { type: "reusableFrom", agent: "claude" }), { claude: { kind: "native" }, codex: { kind: "reusable", from: "claude" } });
  assert.deepEqual(signIns({ type: "needed" }, { type: "unknown" }), { claude: { kind: "required" }, codex: { kind: "required" } });
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
