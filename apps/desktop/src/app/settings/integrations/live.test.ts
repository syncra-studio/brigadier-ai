import assert from "node:assert/strict";
import { test } from "node:test";

import { mergeViews, signIn, snapshotOf } from "@/app/settings/integrations/live";
import type { IntegrationsView, McpServerView, Project, SignInState, SkillView } from "@/ipc/generated";

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
