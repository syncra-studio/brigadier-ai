/** All fifteen screen states on settings.pen's Plugins board. No sample data enters the app. */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { PluginsView } from "@/app/settings/integrations/PluginsPage";
import { connectIntegrations, useIntegrations, type IntegrationAdapter, type IntegrationSnapshot } from "@/app/settings/integrations/adapter";
import type { IntegrationServer, IntegrationSkill } from "@/app/settings/integrations/types";
import { revealSetting, useReveal } from "@/app/settings/reveal";
import { SettingsTitlebarStrip, TerminalButton, TitlebarNav } from "@/components/shared";
import { TooltipProvider } from "@/components/overlays";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
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
        "kind": "required"
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
  }
];

const SERVER_STATES = ["servers", "search", "linear", "pencil", "unreadable", "empty-servers", "long-servers", "narrow-servers"];
const SKILL_STATES = ["skills", "grill-me", "delegator", "claude-skills", "claude-skill", "empty-skills", "narrow-skills"];

function snapshotFor(state: string): IntegrationSnapshot {
  const snapshot: IntegrationSnapshot = {
    catalog: { servers: structuredClone(servers), skills: structuredClone(skills), trustedProjectCount: 3, untrustedProjects: ["scratch", "old-landing"] },
    switches: { mcpServersOff: [servers[4]!.id], skillsOff: [skills[4]!.id] },
  };
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
  if (state === "delegator") snapshot.catalog.skills[1]!.description += ".";
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
  const tab = SKILL_STATES.includes(state) ? "skills" as const : "servers" as const;
  const name = state === "claude-skill" ? "higgsfield-websites" : state === "unreadable" ? "figma-dev" : state;
  const item = (tab === "skills" ? skills : servers).find((entry) => entry.name === name);
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
const emit = () => listeners.forEach((listener) => listener());
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
  showPath: async (path) => { shown.push(path); },
};
connectIntegrations(adapter);

const pause = () => new Promise<void>((resolve) => setTimeout(resolve, 60));
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
function readState() {
  const tile = document.querySelector('[data-slot="integration-tile"]');
  const style = tile ? getComputedStyle(tile) : null;
  return {
    title: text(document.querySelector("main h1")),
    sections: all("main h2").map(text),
    text: text(main()),
    tiles: tileNames(),
    switches: switches(),
    tile: style ? { padding: style.padding.split(" ").map(parseFloat), radius: parseFloat(style.borderRadius), gap: parseFloat(style.gap) } : null,
    overflow: main().scrollWidth > main().clientWidth,
  };
}

async function drive(change: (state: string) => void) {
  const seen: Record<string, unknown> = {};
  const openState = async (state: string) => {
    useReveal.setState({ label: null });
    current = snapshotFor(state);
    emit();
    change(state);
    await pause();
  };
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
    current = { ...current, catalog: { ...current.catalog, servers: current.catalog.servers.map((server) => server.name === "linear" ? { ...server, name: "linear team", signIn: { claude: { kind: "required" }, codex: { kind: "required" } } } : server) } };
    emit(); await pause();
    seen.unsigned = readState();
  }
  await openState(skillMode ? "skills" : "servers");
  current = { ...current, catalog: { ...current.catalog, ...(skillMode ? { skills: current.catalog.skills.slice(1) } : { servers: current.catalog.servers.slice(1) }) } };
  emit(); await pause();
  seen.refreshed = tileNames();
  seen.writes = writes;
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
