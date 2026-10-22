/** All fifteen screen states on settings.pen's Plugins board, and the Plugins · Signing in board's, and a plugin's skill. No sample data enters the app. */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { PluginsView } from "@/app/settings/integrations/PluginsPage";
import { connectIntegrations, useIntegrations, type IntegrationAdapter, type IntegrationSnapshot } from "@/app/settings/integrations/adapter";
import { everywhere, inProject, type SwitchSettings } from "@/app/settings/integrations/live";
import type { IntegrationProject, IntegrationServer, IntegrationSignIn, IntegrationSkill } from "@/app/settings/integrations/types";
import { revealSetting, useReveal } from "@/app/settings/reveal";
import { SettingsTitlebarStrip, TerminalButton, TitlebarNav } from "@/components/shared";
import { TooltipProvider } from "@/components/overlays";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
import type { McpSignInStatus } from "@/ipc/generated";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const servers: IntegrationServer[] = [
  {
    "id": "CursorUser:figma-dev",
    "name": "figma-dev",
    "sources": [
      "CursorUser"
    ],
    "transport": {
      "kind": "http",
      "url": "ws://127.0.0.1:3845/mcp"
    },
    "env": [],
    "headers": [],
    "signIn": {},
    "nativeAgents": [],
    "canSwitchOff": true,
    "error": "Unknown type “websocket”",
    "sourceFiles": [
      {
        "source": "CursorUser",
        "path": "~/.cursor/mcp.json",
        "line": 22
      }
    ]
  },
  {
    "id": "ClaudeUser:linear",
    "name": "linear",
    "sources": [
      "ClaudeUser"
    ],
    "transport": {
      "kind": "http",
      "url": "https://mcp.linear.app/mcp"
    },
    "env": [],
    "headers": [],
    "signIn": {
      "claude": {
        "kind": "native"
      },
      "codex": {
        "kind": "reusable",
        "from": "claude"
      }
    },
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "CodexUser:node_repl",
    "name": "node_repl",
    "sources": [
      "CodexUser"
    ],
    "transport": {
      "kind": "stdio",
      "command": "npx",
      "args": [
        "-y",
        "@openai/node-repl-mcp"
      ]
    },
    "env": [],
    "headers": [],
    "signIn": {},
    "nativeAgents": [
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "ClaudeUser:pencil",
    "name": "pencil",
    "sources": [
      "ClaudeUser",
      "CodexUser",
      "Desktop"
    ],
    "transport": {
      "kind": "stdio",
      "command": "/Applications/Pencil.app/Contents/MacOS/pencil",
      "args": [
        "mcp"
      ]
    },
    "env": [],
    "headers": [],
    "signIn": {},
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "Desktop:playwright",
    "name": "playwright",
    "sources": [
      "Desktop"
    ],
    "transport": {
      "kind": "stdio",
      "command": "npx",
      "args": [
        "@playwright/mcp@latest"
      ]
    },
    "env": [],
    "headers": [],
    "signIn": {},
    "nativeAgents": [],
    "canSwitchOff": true
  },
  {
    "id": "ClaudeUser:railway",
    "name": "railway",
    "sources": [
      "ClaudeUser",
      "CodexUser"
    ],
    "transport": {
      "kind": "stdio",
      "command": "npx",
      "args": [
        "-y",
        "@railway/mcp-server"
      ]
    },
    "env": [],
    "headers": [],
    "signIn": {},
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "ClaudeProjectFile:postgres",
    "name": "postgres",
    "sources": [
      "ClaudeProjectFile"
    ],
    "transport": {
      "kind": "stdio",
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-postgres",
        "postgresql://localhost/brigadier_dev"
      ]
    },
    "env": [],
    "headers": [],
    "signIn": {},
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true,
    "project": "~/Code/brigadier-ai"
  }
];
// The stable fixture ids select the IdGlyph shapes drawn in Pen.
const skills: IntegrationSkill[] = [
  {
    "id": "user:dataviz:18",
    "name": "dataviz",
    "description": "Charts, dashboards and stat tiles that read as one system",
    "locations": [
      "~/.agents/skills/dataviz"
    ],
    "nativeAgents": [
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:delegator:1",
    "name": "delegator",
    "description": "Hand a task to the right worker and check what comes back",
    "locations": [
      "~/.claude/skills/delegator"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:docx:10",
    "name": "docx",
    "description": "Create, read and edit Word documents and templates",
    "locations": [
      "~/.agents/skills/docx"
    ],
    "nativeAgents": [
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:grill-me:1",
    "name": "grill-me",
    "description": "Interview you about a plan until every decision is pinned down",
    "locations": [
      "~/.agents/skills/grill-me",
      "~/.claude/skills/grill-me"
    ],
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true,
    "locationDetails": [
      {
        "path": "~/.agents/skills/grill-me",
        "agents": [
          "codex"
        ]
      },
      {
        "path": "~/.claude/skills/grill-me",
        "linkedTo": "~/.agents/skills/grill-me",
        "agents": [
          "claude"
        ]
      }
    ]
  },
  {
    "id": "user:grilling:3",
    "name": "grilling",
    "description": "Grill you relentlessly about a plan, decision or idea",
    "locations": [
      "~/.claude/skills/grilling"
    ],
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:hallmark:2",
    "name": "hallmark",
    "description": "Anti-AI-slop design for new pages, audits and redesigns",
    "locations": [
      "~/.claude/skills/hallmark"
    ],
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:higgsfield-brandkit:15",
    "name": "higgsfield-brandkit",
    "description": "Visual brand systems: palettes, logo marks, mockups and brandbooks",
    "locations": [
      "~/.claude/skills/higgsfield-brandkit"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:higgsfield-generate:57",
    "name": "higgsfield-generate",
    "description": "Generate images, video, 3D and audio with Higgsfield AI",
    "locations": [
      "~/.claude/skills/higgsfield-generate"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:higgsfield-product-photoshoot:9",
    "name": "higgsfield-product-photoshoot",
    "description": "Brand-quality product images with Higgsfield prompt enhancement",
    "locations": [
      "~/.claude/skills/higgsfield-product-photoshoot"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:higgsfield-websites:47",
    "name": "higgsfield-websites",
    "description": "Build, edit and deploy full-stack sites, apps and games",
    "locations": [
      "~/.claude/skills/higgsfield-websites"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:higgsfield-youtube-thumbnail:13",
    "name": "higgsfield-youtube-thumbnail",
    "description": "High-click-through YouTube thumbnails and video covers",
    "locations": [
      "~/.claude/skills/higgsfield-youtube-thumbnail"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:impeccable:12",
    "name": "impeccable",
    "description": "Design, critique and polish frontend interfaces",
    "locations": [
      "~/.claude/skills/impeccable"
    ],
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:pdf:5",
    "name": "pdf",
    "description": "Read, merge, split, fill and create PDF files",
    "locations": [
      "~/.agents/skills/pdf"
    ],
    "nativeAgents": [
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:security-review:58",
    "name": "security-review",
    "description": "Review pending changes on the branch for security issues",
    "locations": [
      "~/.claude/skills/security-review"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:skill-creator:11",
    "name": "skill-creator",
    "description": "Create skills, improve them and measure how they perform",
    "locations": [
      "~/.claude/skills/skill-creator"
    ],
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:use-railway:8",
    "name": "use-railway",
    "description": "Operate Railway: projects, services, deploys, domains and logs",
    "locations": [
      "~/.claude/skills/use-railway"
    ],
    "nativeAgents": [
      "claude",
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "user:xlsx:11",
    "name": "xlsx",
    "description": "Open, clean up and build spreadsheets",
    "locations": [
      "~/.agents/skills/xlsx"
    ],
    "nativeAgents": [
      "codex"
    ],
    "canSwitchOff": true
  },
  {
    "id": "project:delegator:16",
    "name": "delegator",
    "description": "Brigadier's own rules for splitting work in this repo",
    "locations": [
      "~/Code/brigadier-ai/.agents/skills/delegator"
    ],
    "nativeAgents": [
      "codex"
    ],
    "canSwitchOff": true,
    "project": "~/Code/brigadier-ai"
  },
  {
    "id": "project:release-notes:4",
    "name": "release-notes",
    "description": "Write release notes from the PRs merged since the last tag",
    "locations": [
      "~/Code/brigadier-ai/.claude/skills/release-notes"
    ],
    "nativeAgents": [
      "claude"
    ],
    "canSwitchOff": true,
    "project": "~/Code/brigadier-ai"
  },
  {
    "id": "codex:plugin:spreadsheets@openai-primary-runtime:skills/chart",
    "name": "spreadsheets:chart",
    "description": "Build charts from a spreadsheet's data",
    "locations": [
      "~/.codex/plugins/cache/openai-primary-runtime/spreadsheets/26.1017.22135/skills/chart"
    ],
    "locationDetails": [
      {
        "path": "~/.codex/plugins/cache/openai-primary-runtime/spreadsheets/26.1017.22135/skills/chart",
        "plugin": "spreadsheets@openai-primary-runtime",
        "agents": ["codex"]
      }
    ],
    "nativeAgents": [
      "codex"
    ],
    "canSwitchOff": true
  }
];

// The Signing in board's remote server that no agent is signed in to.
const notion: IntegrationServer = {
  id: "CodexUser:notion", name: "notion", sources: ["CodexUser"], transport: { kind: "http", url: "https://mcp.notion.com/mcp" }, env: [], headers: [],
  signIn: { claude: { kind: "needed" }, codex: { kind: "needed" } }, nativeAgents: ["codex"], canSwitchOff: true,
};
const SIGNED_ON = Date.UTC(2026, 9, 21, 12);
const native: IntegrationSignIn = { kind: "native" };
const SERVER_STATES = ["servers", "search", "linear", "pencil", "unreadable", "empty-servers", "long-servers", "narrow-servers"];
/** Each sign-in state: linear's or notion's agent states (the board's, then the ones it doesn't draw). */
const SIGN_IN_STATES: Record<string, Partial<IntegrationServer>> = {
  "sign-in-list": {},
  "linear-reusing": { signIn: { claude: native, codex: { kind: "reusable", from: "claude" } } },
  "notion-needed": {},
  "notion-both": { sources: ["ClaudeUser", "CodexUser"], nativeAgents: ["claude", "codex"] },
  "notion-brigadier": { signIn: { claude: { kind: "brigadier", sinceMs: SIGNED_ON }, codex: { kind: "brigadier", sinceMs: SIGNED_ON } } },
  "notion-undated": { signIn: { claude: { kind: "brigadier" }, codex: { kind: "brigadier" } } },
  "notion-restored": { pendingSignIn: "sign-in-restored" },
  "linear-refused": { signIn: { claude: native, codex: { kind: "notAllowed", from: "claude" } } },
  "linear-unknown": { signIn: { claude: native, codex: { kind: "unknown" } } },
  "linear-unchecked": { signIn: { claude: { kind: "needed" }, codex: { kind: "unknown" } } },
  "linear-native": { sources: ["ClaudeUser", "CodexUser"], nativeAgents: ["claude", "codex"], signIn: { claude: native, codex: native } },
  "linear-partial": { signIn: { claude: native, codex: { kind: "needed" } } },
  "linear-not-mac": { signIn: { claude: native, codex: { kind: "unsupported" } } },
};
const SKILL_STATES = ["skills", "grill-me", "delegator", "claude-skills", "claude-skill", "plugin-skill", "empty-skills", "narrow-skills"];

// The Plugins · Where it's used board: the user's projects, steady-app's own two servers and the exceptions it draws.
const PROJECTS: IntegrationProject[] = [
  { id: "brigadier-ai", name: "brigadier-ai", path: "~/Code/brigadier-ai", trusted: true },
  { id: "northwind-site", name: "northwind-site", path: "~/Code/clients/northwind-site", trusted: true },
  { id: "steady-app", name: "steady-app", path: "~/Code/steady-app", trusted: true },
  { id: "scratch", name: "scratch", path: "~/scratch", trusted: false },
  { id: "old-landing", name: "old-landing", path: "~/Code/old-landing", trusted: false },
];
const own = (project: IntegrationProject) => ({ project: project.name, projectId: project.id, projectPath: project.path });
const steadyServer = (name: string, command: string, args: string[], cwd?: string): IntegrationServer => ({
  id: `ClaudeProjectFile:steady-app:${name}`, name, sources: ["ClaudeProjectFile"], ...own(PROJECTS[2]!),
  transport: { kind: "stdio", command, args, ...(cwd ? { cwd } : {}) }, env: [], headers: [], signIn: {}, nativeAgents: ["claude"], canSwitchOff: true,
  sourceFiles: [{ source: "ClaudeProjectFile", path: "~/Code/steady-app/.mcp.json" }],
});
const steadyApi = steadyServer("steady-api", "node", ["./tools/steady-api-mcp/index.js"], "~/Code/steady-app");
const steadyDb = steadyServer("steady-db", "npx", ["-y", "@supabase/mcp-server", "--project-ref", "steady"]);
const ID = (name: string) => [...servers, steadyApi, steadyDb, ...skills].find((item) => item.name === name)!.id;
/** Each where state: its tab, the item whose page it opens and the project picked in the toolbar. */
const WHERE_STATES: Record<string, { tab?: "skills"; name?: string; project?: string }> = {
  "where-servers": {},
  "where-skills": { tab: "skills" },
  "where-railway": { name: "railway" },
  "where-pencil": { name: "pencil" },
  "where-grilling": { tab: "skills", name: "grilling" },
  "where-postgres": { name: "postgres" },
  "where-steady-api": { name: "steady-api" },
  "where-playwright": { name: "playwright" },
  "where-untrusted": { name: "pencil" },
  "where-runs-from": { name: "steady-api" },
  "where-brigadier": { project: "brigadier-ai" },
  "where-brigadier-skills": { tab: "skills", project: "brigadier-ai" },
  "where-steady-app": { project: "steady-app" },
  "where-linear-off": { name: "linear", project: "brigadier-ai" },
  "where-linear-all": { name: "linear" },
};

function whereSnapshot(state: string): IntegrationSnapshot {
  const users = structuredClone(servers.slice(0, 6));
  const pencil = users.find((server) => server.name === "pencil")!;
  if (state === "where-pencil" || state === "where-untrusted") pencil.transport = { kind: "stdio", command: "/Applications/Pencil.app/Contents/MacOS/pencil", args: ["mcp", "--stdio"] };
  const postgres: IntegrationServer = { ...structuredClone(servers[6]!), ...own(PROJECTS[0]!), sourceFiles: [{ source: "ClaudeProjectFile", path: "~/Code/brigadier-ai/.mcp.json" }] };
  const api = structuredClone(steadyApi);
  // A working folder that isn't the project's root.
  if (state === "where-runs-from" && api.transport.kind === "stdio") api.transport.cwd = "~/Code/steady-app/tools/steady-api-mcp";
  const userSkills = structuredClone(skills.filter((skill) => !skill.project));
  const grilling = userSkills.find((skill) => skill.name === "grilling")!;
  if (state === "where-grilling") {
    Object.assign(grilling, {
      description: "Grill you relentlessly about a plan, decision or idea.",
      locations: ["~/.agents/skills/grilling", "~/.claude/skills/grilling"],
      locationDetails: [{ path: "~/.agents/skills/grilling", agents: ["codex"] }, { path: "~/.claude/skills/grilling", linkedTo: "~/.agents/skills/grilling", agents: ["claude"] }],
    });
  }
  const projectSkills = structuredClone(skills.filter((skill) => skill.project)).map((skill) => ({ ...skill, ...own(PROJECTS[0]!) }));
  const snapshot: IntegrationSnapshot = {
    catalog: {
      servers: [...users, api, postgres, structuredClone(steadyDb)],
      skills: [...userSkills, ...projectSkills],
      trustedProjectCount: 3,
      untrustedProjects: ["scratch", "old-landing"],
      projects: structuredClone(PROJECTS),
      projectCatalogs: {
        "brigadier-ai": { servers: [...users, postgres, api], skills: [...userSkills, ...projectSkills] },
        "northwind-site": { servers: [...users, api], skills: userSkills },
        "steady-app": { servers: [...users, api, structuredClone(steadyDb)], skills: userSkills },
      },
    },
    switches: {
      mcpServersOff: [ID("playwright")],
      skillsOff: [ID("grilling")],
      projectSwitches: [
        { projectId: "brigadier-ai", serversOn: [], serversOff: [ID("steady-api")], skillsOn: [ID("grilling")], skillsOff: [] },
        { projectId: "steady-app", serversOn: [], serversOff: [ID("pencil")], skillsOn: [ID("grilling")], skillsOff: [] },
      ],
      serversEverywhere: [ID("steady-api")],
      skillsEverywhere: [],
    },
  };
  // An exception kept for a project that is no longer trusted.
  if (state === "where-untrusted") snapshot.switches.projectSwitches!.push({ projectId: "old-landing", serversOn: [], serversOff: [ID("pencil")], skillsOn: [], skillsOff: [] });
  // linear, signed in and on by default, turned off in brigadier-ai.
  if (state.startsWith("where-linear")) snapshot.switches.projectSwitches![0]!.serversOff.push(ID("linear"));
  return snapshot;
}

function snapshotFor(state: string): IntegrationSnapshot {
  if (state in WHERE_STATES) return whereSnapshot(state);
  const snapshot: IntegrationSnapshot = {
    catalog: { servers: structuredClone(servers), skills: structuredClone(skills), trustedProjectCount: 3, untrustedProjects: ["scratch", "old-landing"] },
    switches: { mcpServersOff: [servers[4]!.id], skillsOff: [skills[4]!.id] },
  };
  if (state in SIGN_IN_STATES) {
    snapshot.catalog.servers.splice(3, 0, structuredClone(notion));
    const target = snapshot.catalog.servers.find((server) => server.name === (state.startsWith("linear") ? "linear" : "notion"))!;
    Object.assign(target, structuredClone(SIGN_IN_STATES[state]));
  }
  if (state === "empty-servers") snapshot.catalog.servers = [];
  if (state === "empty-skills") snapshot.catalog.skills = [];
  if (state.startsWith("empty")) snapshot.catalog.untrustedProjects = [];
  if (state.startsWith("claude")) {
    snapshot.catalog.skills.forEach((skill) => { skill.canSwitchOff = !skill.nativeAgents.includes("claude"); });
    // This frame keeps grilling switchable while the other Claude skills are locked on.
    snapshot.catalog.skills[4]!.canSwitchOff = true;
    snapshot.switches.skillsOff.push(skills[9]!.id);
  }
  if (state === "pencil") snapshot.catalog.servers[3]!.transport = { kind: "stdio", command: "/Applications/Pencil.app/Contents/MacOS/pencil", args: ["mcp", "--stdio"] };
  if (state === "grill-me") snapshot.catalog.skills[3]!.description = "Interview you about a plan until every decision in it is pinned down, one question at a time.";
  if (state === "delegator") {
    snapshot.catalog.skills[1]!.description += ".";
    // Claude finds both delegators by itself, so switching one off for Claude hides both.
    snapshot.catalog.skills[1]!.claudeOffByName = true;
  }
  if (state === "claude-skill") snapshot.catalog.skills[9]!.description = "Build, edit and deploy full-stack sites, apps and games with Higgsfield.";
  if (state === "long-servers") {
    const [first, , third, fourth, , , seventh] = snapshot.catalog.servers;
    Object.assign(first!, { name: "acme-internal-observability-gateway", error: undefined, transport: { kind: "stdio", command: "uvx", args: ["--from", "git+https://github.com/acme-corp/observability-mcp@release/2026-10", "acme-obs-mcp", "--region", "eu-west-1"] } });
    Object.assign(third!, { name: "northwind-warehouse-inventory-sync", transport: { kind: "stdio", command: "/Users/stephen/Code/clients/northwind/tools/inventory-mcp/target/release/inventory-mcp", args: ["--stdio"] } });
    Object.assign(fourth!, { name: "sentry-self-hosted-eu", transport: { kind: "stdio", command: "https://sentry.internal.northwind-traders.example.com/api/0/mcp/organizations/northwind/", args: [] } });
    Object.assign(seventh!, { name: "postgres-analytics-replica-readonly", project: "~/Code/clients/northwind/northwind-traders-mobile-monorepo-2026-q4-migration", transport: { kind: "stdio", command: "npx", args: ["-y", "@modelcontextprotocol/server-postgres", "postgresql://readonly@analytics-replica.internal:5432/warehouse"] } });
  }
  return snapshot;
}
function viewFor(state: string) {
  const where = WHERE_STATES[state];
  if (where) return { initialTab: where.tab ?? "servers" as const, ...(where.name ? { initialId: ID(where.name) } : {}), ...(where.project ? { initialProject: where.project } : {}) };
  const tab = SKILL_STATES.includes(state) ? "skills" as const : "servers" as const;
  const name = state === "claude-skill" ? "higgsfield-websites" : state === "plugin-skill" ? "spreadsheets:chart" : state === "unreadable" ? "figma-dev" : state in SIGN_IN_STATES ? state.split("-")[0] : state;
  const item = (tab === "skills" ? skills : snapshotFor(state).catalog.servers).find((entry) => entry.name === name);
  return { initialTab: tab, ...(item ? { initialId: item.id } : {}) };
}

mockIPC(() => null);
document.documentElement.dataset.platform = "macos";
useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  selection: { type: "settings", page: "plugins" },
}));
const initialState = params.get("state") ?? "servers";
let current = snapshotFor(initialState);
const listeners = new Set<() => void>();
const writes: { kind: string; id: string; enabled: boolean }[] = [];
const shown: string[] = [];
// The sign-in IPC the page sent, and how each sign-in the fake daemon started is going.
const calls: string[] = [];
const statuses = new Map<string, McpSignInStatus>();
// Sign-ins the fake daemon forgot (it restarted), and how many status reads fail on a dropped connection first.
const gone = new Set<string>();
let flaky = 0;
let signIns = 0;
const emit = () => listeners.forEach((listener) => listener());
/** The exception and everywhere edits the page sent, each applied as live.ts applies it to Settings. */
const edits: string[] = [];
function editSwitches(log: string, change: (settings: SwitchSettings) => SwitchSettings) {
  edits.push(log);
  const { mcpServersOff, skillsOff, projectSwitches = [], serversEverywhere = [], skillsEverywhere = [] } = current.switches;
  current = { ...current, switches: change({ mcpServersOff, skillsOff, projectSwitches, serversEverywhere, skillsEverywhere }) };
  emit();
}
const forget = (signInId: string) => { if (gone.has(signInId)) throw new Error(`no sign-in ${signInId}`); };
const adapter: IntegrationAdapter = {
  read: async () => current,
  subscribe: (listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
  setServerEnabled: async (id, enabled) => {
    writes.push({ kind: "server", id, enabled });
    current = { ...current, switches: { ...current.switches, mcpServersOff: enabled ? current.switches.mcpServersOff.filter((off) => off !== id) : [...current.switches.mcpServersOff, id] } };
    emit();
  },
  setSkillEnabled: async (id, enabled) => {
    writes.push({ kind: "skill", id, enabled });
    current = { ...current, switches: { ...current.switches, skillsOff: enabled ? current.switches.skillsOff.filter((off) => off !== id) : [...current.switches.skillsOff, id] } };
    emit();
  },
  setInProject: async (kind, ids, projectId, enabled) => editSwitches(`inProject ${kind} ${ids.join(",")} ${projectId} ${enabled}`, (settings) => inProject(settings, kind, ids, projectId, enabled)),
  setEverywhere: async (kind, ids, origin, on) => editSwitches(`everywhere ${kind} ${ids.join(",")} ${origin} ${on}`, (settings) => everywhere(settings, kind, ids, origin, on)),
  showPath: async (path) => { shown.push(path); },
  signIn: async (id) => {
    const signInId = `sign-in-${++signIns}`;
    calls.push(`signInMcpServer ${id}`);
    statuses.set(signInId, { type: "pending" });
    return signInId;
  },
  signInStatus: async (signInId) => {
    if (flaky > 0) { flaky--; throw new Error("connection closed"); }
    forget(signInId);
    return statuses.get(signInId) ?? { type: "pending" };
  },
  cancelSignIn: async (signInId) => { calls.push(`cancelMcpSignIn ${signInId}`); forget(signInId); statuses.set(signInId, { type: "cancelled" }); },
  reopenSignIn: async (signInId) => { calls.push(`reopenMcpSignIn ${signInId}`); forget(signInId); },
  signOut: async (id) => { calls.push(`signOutMcpServer ${id}`); },
  askSignInAgain: async (id) => { calls.push(`askMcpSignInAgain ${id}`); },
};
connectIntegrations(adapter);

const pause = () => new Promise<void>((resolve) => setTimeout(resolve, 60));
// The sign-in status polls each second, in the page's virtual time.
const poll = () => new Promise<void>((resolve) => setTimeout(resolve, 1100));
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const text = (node: Element | null) => node?.textContent?.trim() ?? "";
const main = () => document.querySelector("main")!;
const click = (label: string) => {
  const button = [...all<HTMLButtonElement>("main button"), ...all<HTMLButtonElement>("button")].find((node) => node.getAttribute("aria-label") === label || text(node) === label);
  if (!button) throw new Error(`No button: ${label}`);
  button.click();
};
const input = (label: string, value: string) => {
  const field = all<HTMLInputElement>("input").find((node) => node.getAttribute("aria-label") === label && node.getBoundingClientRect().width > 0);
  if (!field) throw new Error(`No input: ${label}`);
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
};
const tileNames = () => all('[data-slot="integration-tile"] button:first-child').map((node) => node.getAttribute("aria-label"));
const switches = () => all('[role="switch"]').map((node) => ({ label: node.getAttribute("aria-label"), checked: node.getAttribute("aria-checked"), disabled: node.hasAttribute("disabled") }));
/** The Sign-in section: its status row and each agent's row. */
const latestSignIn = () => `sign-in-${signIns}`;

function readSignIn() {
  const status = document.querySelector('[data-slot="sign-in-status"]');
  if (!status) return null;
  const [tile, words] = [...status.children];
  return {
    title: text(words!.children[0]!),
    line: text(words!.children[1]!),
    tile: ["bg-success/15", "bg-warning/15", "bg-destructive/10", "bg-foreground/5"].find((name) => tile!.classList.contains(name)),
    spinner: !!tile!.querySelector(".animate-spin"),
    actions: all("button", status).map(text),
    rows: all('[data-slot="worker-row"]', main()).map((row) => {
      const [, labels, end] = [...row.children];
      return { title: text(labels!.children[0]!), line: text(labels!.children[1]!), end: text(end?.querySelector("code") ?? end ?? null) };
    }),
  };
}
function readState() {
  const tile = document.querySelector('[data-slot="integration-tile"]');
  const style = tile ? getComputedStyle(tile) : null;
  return {
    title: text(document.querySelector("main h1")),
    sections: all("main h2").map(text),
    text: text(main()),
    tiles: tileNames(),
    // Tiles whose mark is the orange Key of a server that needs a sign-in.
    keyTiles: all('[data-slot="integration-tile"]').filter((item) => item.querySelector(".bg-warning\\/15")).map((item) => item.getAttribute("data-setting")),
    switches: switches(),
    tile: style ? { padding: style.padding.split(" ").map(parseFloat), radius: parseFloat(style.borderRadius), gap: parseFloat(style.gap) } : null,
    overflow: main().scrollWidth > main().clientWidth,
    description: text(document.querySelector("main h1")?.nextElementSibling ?? null),
    signIn: readSignIn(),
  };
}

/** Where it's used, the toolbar's project picker and the exception marks on the list, as the Where it's used board draws them. */
function readWhere() {
  const section = document.querySelector('main [data-setting="Where it\'s used"]');
  const summary = section?.querySelector('[data-slot="where-used"]');
  const select = document.querySelector('main [data-setting^="project:"]');
  return {
    labels: all("main label").filter((label) => label.querySelector('[role="switch"]')).map(text),
    description: text(section?.querySelector("p") ?? null),
    radios: all('[role="radio"]', section ?? document.createElement("div")).map((radio) => ({ title: text(radio.children[1]!.children[0]!), line: text(radio.children[1]!.children[1]!), checked: radio.getAttribute("aria-checked") })),
    summary: summary ? { title: text(summary.firstElementChild!.children[0]!), line: text(summary.firstElementChild!.children[1]!), button: text(summary.querySelector("button")) } : null,
    exceptions: all('[data-slot="exception-row"]').map((row) => {
      const [name, path] = [...row.children[1]!.children];
      return { name: text(name!), path: text(path!), badge: text(row.children[2]!) };
    }),
    chips: Object.fromEntries(all('[data-slot="integration-tile"]').flatMap((tile) => {
      const chips = all('[data-slot="scope-chip"]', tile).map(text);
      return chips.length ? [[tile.getAttribute("data-setting"), chips]] : [];
    })),
    groups: all("#integrations-list > section").map((group) => ({ title: text(group.querySelector("h2")), description: text(group.querySelector("header p")), aside: text(group.querySelector("header > span")) })),
    // Each tile: its name, its line, and its switch's state or the words in its place.
    lines: all('[data-slot="integration-tile"]').map((tile) => {
      const end = tile.lastElementChild!;
      return [text(tile.querySelector("button > span:nth-child(2) > span:first-child > span:first-child")), text(tile.querySelector("button > span:nth-child(2) > span:last-child")), end.getAttribute("role") === "switch" ? end.getAttribute("aria-checked")! : text(end)];
    }),
    tabs: all('[role="tab"]').map(text),
    select: select ? { setting: select.getAttribute("data-setting"), value: text(select), washed: !!select.querySelector(".bg-foreground\\/10") } : null,
    note: text(document.querySelector("#integrations-list > div.bg-foreground\\/5")),
  };
}
/** The open project picker: its find field, label, each project and its footer. */
function readPicker() {
  const picker = document.querySelector('[role="dialog"]');
  if (!picker) return null;
  return {
    name: picker.getAttribute("aria-label"),
    placeholder: picker.querySelector("input")?.getAttribute("placeholder"),
    label: text(picker.querySelector("p")),
    projects: all("button", picker).map((button) => ({ name: text(button.querySelector("span > span")), path: text(button.querySelector("span > span + span")), state: text(button.querySelector(":scope > span:last-child:not(:nth-child(2))")), disabled: (button as HTMLButtonElement).disabled })),
    footer: text(picker.querySelector("p:last-child")),
    // Bound to the room the window leaves, with only the projects scrolling.
    bounded: getComputedStyle(picker).maxHeight !== "none",
    scrolls: getComputedStyle(picker.querySelector("button")!.parentElement!).overflowY,
  };
}
/** The tooltip a pointer over `node` shows. */
async function hover(node: Element | null | undefined) {
  if (!node) return null;
  node.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, pointerType: "mouse" }));
  await pause();
  const tip = text(document.querySelector('[role="tooltip"]'));
  node.dispatchEvent(new PointerEvent("pointerleave", { bubbles: true, pointerType: "mouse" }));
  return tip;
}
const radio = (title: string) => {
  const node = all<HTMLButtonElement>('[role="radio"]').find((item) => text(item).startsWith(title));
  if (!node) throw new Error(`No radio: ${title}`);
  node.click();
};

const readWhereState = () => ({ ...readState(), where: readWhere() });
const switchesNow = () => structuredClone(current.switches);

/** Every Where it's used state, then its actions: the picker, an exception's X, the radios, the project view's switches and search. */
async function driveWhere(openState: (state: string) => Promise<void>) {
  const seen: Record<string, unknown> = {};
  for (const state of Object.keys(WHERE_STATES)) {
    await openState(state);
    seen[state] = readWhereState();
  }
  // The chip and the X name what they do on hover.
  await openState("where-skills");
  seen.chipTip = await hover(all('[data-slot="scope-chip"]').find((chip) => text(chip) === "On in 2 projects"));
  await openState("where-pencil");
  seen.removeTip = await hover(document.querySelector('[aria-label="Remove steady-app\'s exception"]'));
  // The picker: find a project, then pick one.
  click("Turn off in a project…"); await pause();
  seen.picker = readPicker();
  input("Find a project", "north"); await pause();
  seen.pickerFound = readPicker();
  input("Find a project", ""); await pause();
  const picked = all<HTMLButtonElement>('[role="dialog"] button').find((button) => text(button).startsWith("brigadier-ai"))!;
  picked.click(); await pause();
  seen.picked = { ...readWhereState(), settings: switchesNow() };
  click("Remove steady-app's exception"); await pause();
  seen.removed = { ...readWhereState(), settings: switchesNow() };
  // A project's own item: used in every project, then kept in its project again; on, then off there.
  await openState("where-postgres");
  radio("Every project"); await pause();
  seen.postgresEverywhere = { ...readWhereState(), settings: switchesNow() };
  radio("Only in brigadier-ai"); await pause();
  seen.postgresOnly = { ...readWhereState(), settings: switchesNow() };
  click("postgres"); await pause();
  seen.postgresOff = { ...readWhereState(), settings: switchesNow() };
  radio("Every project"); await pause();
  seen.postgresOffEverywhere = { ...readWhereState(), settings: switchesNow() };
  radio("Only in brigadier-ai"); await pause();
  seen.postgresOffOnly = { ...readWhereState(), settings: switchesNow() };
  await openState("where-steady-api");
  radio("Only in steady-app"); await pause();
  seen.steadyOnly = { ...readWhereState(), settings: switchesNow() };
  // The project view: a switch adds or removes brigadier-ai's exception; its own item's is its on or off there.
  await openState("where-brigadier");
  click("playwright"); await pause();
  click("steady-api"); await pause();
  seen.viewSwitched = { ...readWhereState(), settings: switchesNow() };
  click("postgres"); await pause();
  seen.viewOwn = switchesNow();
  // Settings search finds servers by name with where they live, and the project.
  await openState("where-servers");
  input("Search settings", "steady"); await pause();
  seen.search = all('[data-sidebar="sidebar"] button').map(text).filter((line) => line.startsWith("steady"));
  click("steady-appPlugins · project view"); await pause();
  seen.searchOpened = readWhereState();
  input("Search settings", ""); await pause();
  seen.edits = edits;
  seen.writes = writes;
  return seen;
}

async function drive(change: (state: string) => void) {
  const seen: Record<string, unknown> = {};
  const openState = async (state: string) => {
    useReveal.setState({ label: null });
    useApp.setState((app) => ({ info: { ...app.info, platform: state === "linear-not-mac" ? "linux" : "macos" } as typeof app.info }));
    current = snapshotFor(state);
    emit();
    change(state);
    await pause();
  };
  if (params.get("drive") === "sign-in") return driveSignIn(openState);
  if (params.get("drive") === "where-used") return driveWhere(openState);
  const skillMode = params.get("drive") === "skills";
  for (const state of skillMode ? SKILL_STATES : SERVER_STATES) {
    await openState(state);
    seen[state] = readState();
  }
  await openState(skillMode ? "skills" : "servers");
  if (skillMode) {
    click("delegator"); await pause();
    seen.duplicateSwitch = current.switches.skillsOff.includes(skills[1]!.id) && !current.switches.skillsOff.includes(skills[17]!.id);
    click("Open grill-me"); await pause();
    seen.opened = text(document.querySelector("main h1"));
    click("Show folder"); await pause();
    seen.shown = [...shown];
    click("Plugins"); await pause();
    input("Search skills", "rail"); await pause();
    seen.filtered = tileNames();
    await openState("claude-skill");
    click("higgsfield-websites: Codex workers"); await pause();
    seen.claudeOn = readState();
  } else {
    click("railway"); await pause();
    seen.switchedOff = current.switches.mcpServersOff.includes(servers[5]!.id);
    click("Open linear"); await pause();
    seen.opened = text(document.querySelector("main h1"));
    click("Show file"); await pause();
    seen.shown = [...shown];
    click("Plugins"); await pause();
    input("Search MCP servers", "rail"); await pause();
    seen.filtered = tileNames();
    input("Search MCP servers", ""); await pause();
    input("Search settings", "rail"); await pause();
    seen.searchResults = text(document.querySelector('[data-sidebar="sidebar"]'));
    click("railwayPlugins › MCP servers"); await pause();
    seen.revealed = document.querySelector(`[data-setting="server:${servers[5]!.id}"]`)?.classList.contains("bg-foreground/8");
    await openState("linear");
    current = { ...current, catalog: { ...current.catalog, servers: current.catalog.servers.map((server) => server.name === "linear" ? { ...server, nativeAgents: ["claude", "codex"], signIn: { claude: { kind: "native" }, codex: { kind: "reusable", from: "claude" } } } : server) } };
    emit(); await pause();
    seen.reusable = readState();
    current = { ...current, catalog: { ...current.catalog, servers: current.catalog.servers.map((server) => server.name === "linear" ? { ...server, name: "linear team", signIn: { claude: { kind: "needed" }, codex: { kind: "needed" } } } : server) } };
    emit(); await pause();
    seen.unsigned = readState();
    // Set up only in Claude Code: Codex has no entry its own login could find.
    current = { ...current, catalog: { ...current.catalog, servers: current.catalog.servers.map((server) => server.name === "linear team" ? { ...server, nativeAgents: ["claude"] } : server) } };
    emit(); await pause();
    seen.claudeOnly = readState();
  }
  await openState(skillMode ? "skills" : "servers");
  current = { ...current, catalog: { ...current.catalog, ...(skillMode ? { skills: current.catalog.skills.slice(1) } : { servers: current.catalog.servers.slice(1) }) } };
  emit(); await pause();
  seen.refreshed = tileNames();
  seen.writes = writes;
  return seen;
}

/** Every sign-in state, then the actions: the tile, signing in, failing, cancelling, checking and signing out. */
async function driveSignIn(openState: (state: string) => Promise<void>) {
  const seen: Record<string, unknown> = {};
  const scrolled: boolean[] = [];
  const scroll = Element.prototype.scrollIntoView;
  Element.prototype.scrollIntoView = function (this: Element, ...args: Parameters<Element["scrollIntoView"]>) {
    scrolled.push(!!this.querySelector('[data-slot="sign-in-status"]'));
    scroll.apply(this, args);
  };
  for (const state of Object.keys(SIGN_IN_STATES)) {
    await openState(state);
    seen[state] = readState();
  }
  await openState("sign-in-list");
  click("Open linear"); await pause();
  seen.linearScrolled = [...scrolled];
  click("Plugins"); await pause();
  click("Open notion"); await pause();
  seen.notionOpened = readState();
  seen.notionScrolled = [...scrolled];
  click("Sign in in Brigadier"); await pause();
  seen.signing = readState();
  click("Open the page again"); await pause();
  statuses.set("sign-in-1", { type: "failed", reason: "The server didn't accept the sign-in." });
  await poll();
  seen.failed = readState();
  click("Try again"); await pause();
  statuses.set("sign-in-2", { type: "failed", reason: "Timed out after 5 minutes. Nothing was saved." });
  await poll();
  seen.timedOut = readState();
  click("Try again"); await pause();
  click("Cancel"); await pause();
  seen.cancelledNeeded = readState();
  await openState("linear-refused");
  click("Ask macOS again"); await pause();
  click("Sign in in Brigadier"); await pause();
  seen.signingRefused = readState();
  click("Cancel"); await pause();
  seen.cancelledRefused = readState();
  await openState("linear-unknown");
  click("Check now"); await pause();
  await openState("notion-brigadier");
  click("Sign out"); await pause();
  // Settings reopened while a sign-in waits: Waiting comes back, then the daemon reports it done.
  await openState("notion-restored");
  seen.restored = readState();
  current = snapshotFor("notion-brigadier");
  statuses.set("sign-in-restored", { type: "signedIn" });
  await poll();
  seen.restoredDone = readState();
  seen.calls = [...calls];
  // A restored attempt keeps waiting after the catalog drops its id, until its end is read.
  statuses.delete("sign-in-restored");
  await openState("notion-needed");
  await openState("notion-restored");
  current = snapshotFor("notion-needed");
  emit(); await pause();
  seen.droppedWaiting = readSignIn();
  statuses.set("sign-in-restored", { type: "failed", reason: "Timed out after 5 minutes. Nothing was saved." });
  await poll();
  seen.droppedFailed = readSignIn();
  // A dropped connection keeps the attempt waiting; an attempt the daemon no longer knows has ended.
  await openState("notion-needed");
  click("Sign in in Brigadier"); await pause();
  flaky = 1;
  await poll();
  seen.flakyWaiting = readSignIn();
  gone.add(latestSignIn());
  await poll();
  seen.interrupted = readSignIn();
  click("Try again"); await pause();
  gone.add(latestSignIn());
  click("Cancel"); await pause();
  seen.cancelledGone = { ...readSignIn(), alert: text(document.querySelector('[role="alert"]')) };
  click("Sign in in Brigadier"); await pause();
  gone.add(latestSignIn());
  click("Open the page again"); await pause();
  seen.reopenGone = readSignIn();
  // Switched off, a signed-in server doesn't say every worker can use it.
  await openState("linear-reusing");
  click("linear"); await pause();
  seen.switchedOff = { ...readState(), off: current.switches.mcpServersOff.includes("ClaudeUser:linear") };
  // A sign-in the agent's own way after a failed attempt outdates the failure, whichever lands first.
  const codexSignedIn = () => {
    current = { ...current, catalog: { ...current.catalog, servers: current.catalog.servers.map((server) => server.name === "linear" ? { ...server, signIn: { claude: native, codex: native } } : server) } };
    emit();
  };
  await openState("linear-partial");
  click("Sign in in Brigadier"); await pause();
  statuses.set(latestSignIn(), { type: "failed", reason: "The server didn't accept the sign-in." });
  await poll();
  seen.partialFailed = readSignIn();
  codexSignedIn(); await pause();
  seen.recoveredAfter = readSignIn();
  await openState("linear-reusing");
  await openState("linear-partial");
  click("Sign in in Brigadier"); await pause();
  codexSignedIn(); await pause();
  statuses.set(latestSignIn(), { type: "failed", reason: "The server didn't accept the sign-in." });
  await poll();
  seen.recoveredBefore = readSignIn();
  seen.reviewCalls = calls.slice((seen.calls as string[]).length);
  return seen;
}

function Fixture() {
  const [state, setState] = useState(initialState);
  const [result, setResult] = useState("");
  const { snapshot, error } = useIntegrations();
  useEffect(() => {
    if (params.has("drive")) {
      void drive(setState).then((value) => setResult(JSON.stringify(value))).catch((cause: unknown) => setResult(JSON.stringify({ error: String(cause) })));
    } else if (initialState === "search") {
      const timer = setTimeout(() => { input("Search settings", "rail"); revealSetting(`server:${servers[5]!.id}`); }, 100);
      return () => clearTimeout(timer);
    }
  }, []);
  return <TooltipProvider>
    <div className="bg-chrome text-foreground flex h-screen flex-col px-1 pb-1">
      <SidebarProvider keepOpen className="relative min-h-0 flex-1 flex-col">
        <div className="relative flex min-h-0 min-w-0 flex-1">
          <SidebarPanel><SettingsNav /></SidebarPanel>
          <div className="flex min-w-0 flex-1 flex-col">
            <SettingsTitlebarStrip><TerminalButton /></SettingsTitlebarStrip>
            <main className="bg-background border-divider rounded-e-page min-h-0 flex-1 overflow-hidden border-l">
              <PluginsView key={state} snapshot={snapshot} adapter={adapter} error={error} {...viewFor(state)} />
            </main>
          </div>
        </div>
        <TitlebarNav />
      </SidebarProvider>
    </div>
    {result && <pre id="integrations-result">{result}</pre>}
  </TooltipProvider>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
