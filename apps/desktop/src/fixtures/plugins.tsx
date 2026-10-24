/** Settings › Plugins over a faked daemon: every list, details and dialog state the Plugins board draws. No sample data enters the app. */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { connectPlugins, usePlugins, type PluginAgentResult, type PluginsAdapter } from "@/app/settings/plugins/adapter";
import { PluginsPage } from "@/app/settings/plugins/PluginsPage";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
import { Toaster, TooltipProvider } from "@/components/overlays";
import { SettingsTitlebarStrip, TerminalButton, TitlebarNav } from "@/components/shared";
import type { AgentItemState, PluginAttention, PluginItemView, PluginKind, PluginProjectView, PluginsView, ProviderKind } from "@/ipc/generated";
import { useApp } from "@/state/store";

/*
 * ?state=<name> opens one drawn state (L1–L7, D1–D3, G1–G7) for shots; ?agents=2 keeps to the
 * two agents Brigadier drives today (the board draws six); ?drive=1 walks every state, flips
 * switches and confirms the dialogs, then writes what it saw to #plugins-result
 * (PluginsRender.test.ts reads it).
 */
const params = new URLSearchParams(location.search);
if (params.has("drive") || params.has("state")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}
useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info }));

const ALL_AGENTS = [
  ["claude", "Claude Code"],
  ["codex", "Codex"],
  ["gemini", "Gemini CLI"],
  ["cursor", "Cursor Agent"],
  ["opencode", "OpenCode"],
  ["amp", "Amp"],
] as const;
// Agents beyond the two the app drives today have no type of their own yet: the page takes any.
const agents = ALL_AGENTS.slice(0, params.get("agents") === "2" ? 2 : 6).map(([agent, label]) => ({ agent: agent as ProviderKind, label }));
const HOME = "/Users/me/Code";
const BRIGADIER = `${HOME}/brigadier-ai`;
const STEADY = `${HOME}/steady-app`;
const LANDING = `${HOME}/landing`;
const projects: PluginProjectView[] = [
  { id: "p-brigadier", name: "brigadier-ai", path: BRIGADIER, notes: [] },
  { id: "p-steady", name: "steady-app", path: STEADY, notes: [] },
  { id: "p-landing", name: "landing", path: LANDING, notes: [{ agent: "codex", message: "Codex only reads this project's settings once you trust the project." }] },
];

/** One item as the fake daemon keeps it. */
type Spec = {
  key: string;
  kind: PluginKind;
  name: string;
  description: string;
  detail: string;
  remote?: boolean;
  library?: boolean;
  switchable?: boolean;
  everywhere: boolean;
  onIn?: string[];
  offIn?: string[];
  /** The agents that have it; every agent when left out. */
  only?: string[];
  /** Each agent's state where it is on, when not Ready. */
  special?: Record<string, AgentItemState>;
  attention?: PluginAttention;
  foundIn?: string;
};

const wontStart: AgentItemState = { type: "wontStart", reason: 'unknown type "websocket"' };
const needsSignIn: AgentItemState = { type: "needsSignIn" };

function startingItems(signIns: number): Spec[] {
  const waiting = ["codex", "gemini", "cursor", "opencode"].slice(0, signIns);
  return [
    { key: "server:pencil", kind: "server", name: "pencil", description: "Reads and edits .pen design files.", detail: "pencil mcp --stdio", everywhere: true },
    { key: "server:railway", kind: "server", name: "railway", description: "Railway projects, services and deploys.", detail: "railway mcp", everywhere: true, offIn: [STEADY] },
    { key: "server:node_repl", kind: "server", name: "node_repl", description: "Runs JavaScript in a sandbox.", detail: "node-repl-mcp", everywhere: true, offIn: [BRIGADIER] },
    {
      key: "server:linear", kind: "server", name: "linear", description: "Issues and projects in Linear.", detail: "https://mcp.linear.app/mcp", remote: true, everywhere: true,
      special: { ...Object.fromEntries(waiting.map((agent) => [agent, needsSignIn])), amp: { type: "notSupported", reason: "Amp can't sign in to servers on the web yet." } },
      attention: { type: "signIn", message: signIns === 1 ? "Codex needs you to sign in." : "Codex, Gemini CLI, Cursor Agent and OpenCode need you to sign in." },
    },
    {
      key: "server:figma-dev", kind: "server", name: "figma-dev", description: "Figma's local server.", detail: "ws://localhost:3845/mcp", remote: true, everywhere: true,
      special: Object.fromEntries(ALL_AGENTS.map(([agent]) => [agent, wontStart])), attention: { type: "wontStart", message: 'Won\'t start: unknown type "websocket".' },
    },
    { key: "server:playwright", kind: "server", name: "playwright", description: "Drives a browser.", detail: "npx @playwright/mcp@latest", everywhere: false },
    { key: "lib:postgres", kind: "server", name: "postgres", description: "Your local brigadier database.", detail: "npx @modelcontextprotocol/server-postgres", library: true, everywhere: false, onIn: [BRIGADIER] },
    { key: "plugin:higgsfield/higgsfield", kind: "skill", name: "higgsfield", description: "Images, video, brand kits and websites.", detail: "~/.claude/plugins/higgsfield", everywhere: true },
    { key: "skill:impeccable", kind: "skill", name: "impeccable", description: "Designs, critiques and polishes interfaces.", detail: "~/.agents/skills/impeccable", everywhere: true },
    { key: "skill:hallmark", kind: "skill", name: "hallmark", description: "Keeps new pages free of generic AI styling.", detail: "~/.agents/skills/hallmark", everywhere: true, offIn: [BRIGADIER] },
    { key: "skill:use-railway", kind: "skill", name: "use-railway", description: "Deploys and runs things on Railway.", detail: "~/.agents/skills/use-railway", everywhere: true, offIn: [STEADY] },
    {
      key: "skill:grill-me", kind: "skill", name: "grill-me", description: "Asks hard questions about a plan.", detail: "~/.claude/skills/grill-me", everywhere: true, only: ["claude"], switchable: false,
      attention: { type: "notEverywhere", message: "Only Claude Code has it." },
    },
    { key: "lib:delegator", kind: "skill", name: "delegator", description: "Runs your plan with workers while you're away.", detail: "~/.brigadier/library/delegator", library: true, everywhere: false, onIn: [BRIGADIER] },
  ];
}

const elsewhere: Spec[] = [
  { key: "other:desktop:filesystem", kind: "server", name: "filesystem", description: "Reads and writes files in folders you pick.", detail: "npx @modelcontextprotocol/server-filesystem", everywhere: false, only: [], foundIn: "Claude Desktop" },
  { key: "other:desktop:brave-search", kind: "server", name: "brave-search", description: "Searches the web with Brave.", detail: "https://search.brave.com/mcp", remote: true, everywhere: false, only: [], foundIn: "Claude Desktop" },
  { key: "other:cursor:context7", kind: "server", name: "context7", description: "Up-to-date library docs.", detail: "https://mcp.context7.com/mcp", remote: true, everywhere: false, only: [], foundIn: "Cursor" },
];

let items: Spec[] = startingItems(1);
let empty = false;
const calls: string[] = [];
let adding: { resolve: (results: PluginAgentResult[]) => void } | null = null;

function view(projectId: string | null): PluginsView {
  const focus = projects.find((project) => project.id === projectId)?.path ?? null;
  const shown = (spec: Spec): PluginItemView => {
    const on = focus ? (spec.everywhere ? !(spec.offIn ?? []).includes(focus) : (spec.onIn ?? []).includes(focus)) : spec.everywhere;
    return {
      key: spec.key, kind: spec.kind, name: spec.name, description: spec.description, detail: spec.detail, remote: spec.remote ?? false,
      library: spec.library ?? false, everywhere: spec.everywhere, onIn: spec.everywhere ? [] : spec.onIn ?? [], offIn: spec.everywhere ? spec.offIn ?? [] : [],
      agents: agents.map(({ agent }) => ({
        agent,
        state: spec.only && !spec.only.includes(agent) ? { type: "missing" } : spec.special?.[agent] && (on || spec.special[agent]!.type === "working") ? spec.special[agent]! : on ? { type: "ready" } : { type: "off" },
      })),
      attention: spec.attention ?? null, switchable: spec.switchable ?? true, updateAvailable: false, canUndo: false, files: [`${HOME}/.claude.json`], foundIn: spec.foundIn ?? null,
    };
  };
  if (empty) return { agents, projects, items: [], elsewhere: [] };
  return { agents, projects, items: items.map(shown), elsewhere: elsewhere.map(shown) };
}

const ok = (): PluginAgentResult[] => agents.map(({ agent }) => ({ agent, ok: true, message: null }));
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((listener) => listener());
const spec = (key: string) => items.find((item) => item.key === key) ?? elsewhere.find((item) => item.key === key);
const toggle = (list: string[] = [], path: string, add: boolean) => (add ? [...new Set([...list, path])] : list.filter((entry) => entry !== path));

const adapter: PluginsAdapter = {
  read: async (projectId) => view(projectId),
  subscribe: (onChange) => {
    listeners.add(onChange);
    return () => listeners.delete(onChange);
  },
  switchPlugin: async (key, projectId, on) => {
    calls.push(`switchPlugin ${key} ${projectId ?? "everywhere"} ${on ? "on" : "off"}`);
    const item = spec(key)!;
    const path = projects.find((project) => project.id === projectId)?.path;
    if (!path) Object.assign(item, on ? { everywhere: true, offIn: [], onIn: [] } : { everywhere: false, onIn: [], offIn: [] });
    else if (item.everywhere) item.offIn = toggle(item.offIn, path, !on);
    else item.onIn = toggle(item.onIn, path, on);
    changed();
    return ok();
  },
  checkAdd: async (text) => {
    calls.push(`checkPluginAdd ${text}`);
    if (!text.includes("sentry")) return [{ name: "", kind: "server", detail: "", remote: false, signsIn: false, error: "That doesn't look like an MCP server or a skill." }];
    return [{ name: "sentry", kind: "server", detail: "https://mcp.sentry.dev/mcp", remote: true, signsIn: true, error: null }];
  },
  add: (_text, name, projectId, shared) => {
    calls.push(`addPlugin ${name ?? "-"} ${projectId ?? "everywhere"} ${shared ? "shared" : "just me"}`);
    // Claude Code and Codex have it already; the next is adding it.
    items.push({
      key: "lib:sentry", kind: "server", name: "sentry", description: "Errors and issues in Sentry.", detail: "https://mcp.sentry.dev/mcp", remote: true, library: true,
      everywhere: projectId === null, onIn: projectId ? [projects.find((project) => project.id === projectId)!.path] : [],
      special: Object.fromEntries(agents.slice(2).map(({ agent }) => [agent, { type: "missing" } as AgentItemState])),
      only: agents.slice(0, 2).map(({ agent }) => agent),
    });
    changed();
    return new Promise((resolve) => {
      adding = { resolve };
    });
  },
  everyAgent: async (key) => {
    calls.push(`useEverywhere ${key}`);
    const item = spec(key)!;
    Object.assign(item, { key: `lib:${item.name}`, library: true, only: undefined, attention: undefined, switchable: true });
    changed();
    return ok();
  },
  undoEveryAgent: async (key) => {
    calls.push(`undoUseEverywhere ${key}`);
    return ok();
  },
  remove: async (key) => {
    calls.push(`removePlugin ${key}`);
    items = items.filter((item) => item.key !== key);
    changed();
    return ok();
  },
  update: async (key) => {
    calls.push(`updatePlugin ${key}`);
    return ok();
  },
  signIn: async (key, projectId) => {
    calls.push(`signInPlugin ${key} ${projectId ?? "everywhere"}`);
    return new Promise(() => {});
  },
  cancelSignIn: async (key) => {
    calls.push(`cancelPluginSignIn ${key}`);
  },
  pickFolder: async () => {
    calls.push("pickFolder");
    return null;
  },
  trustProject: async (projectId) => {
    calls.push(`trustProject ${projectId}`);
  },
  addProject: () => calls.push("addProject"),
};
connectPlugins(adapter);

const pause = (ms = 60) => new Promise<void>((resolve) => setTimeout(resolve, ms));
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const text = (node: Element | null | undefined) => node?.textContent?.trim() ?? "";
const main = () => document.querySelector("main")!;
function click(label: string, root: ParentNode = document) {
  const button = all<HTMLButtonElement>("button, [role=switch]", root).find((node) => node.getAttribute("aria-label") === label || text(node) === label);
  if (!button) throw new Error(`No button: ${label}`);
  button.click();
}
function type(label: string, value: string) {
  const field = all<HTMLTextAreaElement | HTMLInputElement>("textarea, input").find((node) => node.getAttribute("aria-label") === label);
  if (!field) throw new Error(`No field: ${label}`);
  const proto = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}
/** The switch of row `name` in the list (the first such row). */
function flip(name: string) {
  const row = all('[data-slot="plugin-row"]', main()).find((node) => text(node.querySelector(".font-medium")) === name && node.querySelector("[role=switch]"));
  if (!row) throw new Error(`No row with a switch: ${name}`);
  row.querySelector<HTMLButtonElement>("[role=switch]")!.click();
}

function rowsOf(root: ParentNode) {
  return all('[data-slot="plugin-row"]', root).map((row) => {
    const line = row.querySelector('[data-slot="row-line"]');
    return {
      name: text(row.querySelector(".font-medium")),
      line: text(line),
      tone: line?.getAttribute("data-tone") ?? null,
      off: row.hasAttribute("data-off"),
      lit: all('[data-slot="agent-mark"]', row).filter((mark) => !mark.hasAttribute("data-dim")).map((mark) => mark.getAttribute("data-agent")),
      chips: all('[data-slot="scope-chip"]', row).map(text),
      switch: row.querySelector("[role=switch]")?.getAttribute("aria-checked") ?? null,
      button: text(row.querySelector("button[data-slot=button]")) || null,
    };
  });
}

function read() {
  const column = document.querySelector('[data-slot="settings-page"] h1')?.closest(".mx-auto");
  const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className: "max-w-settings" }));
  const settingsColumn = Boolean(column) && getComputedStyle(column as Element).maxWidth === getComputedStyle(probe).maxWidth;
  probe.remove();
  const dialog = document.querySelector<HTMLElement>("[data-dialog]");
  const menu = document.querySelector('[data-slot="project-filter"]');
  return {
    settingsColumn,
    title: text(document.querySelector("main h1")),
    description: text(document.querySelector("main h1")?.nextElementSibling),
    sections: all("main section").map((section) => ({
      title: text(section.querySelector("h2")),
      description: text(section.querySelector("h2")?.nextElementSibling),
      count: text(section.querySelector("h2")?.parentElement?.nextElementSibling),
      rows: rowsOf(section),
    })),
    tabs: all('main [role="tab"]').map((tab) => `${text(tab)}${tab.getAttribute("aria-selected") === "true" ? " *" : ""}`),
    filter: text(document.querySelector('main [data-slot="plugins-toolbar"] button')),
    folds: all('main [data-slot="fold"]').map((fold) => ({ text: text(fold.querySelector("button")), open: fold.querySelector("button")?.getAttribute("aria-expanded") === "true", rows: rowsOf(fold) })),
    agentRows: all('main [data-slot="agent-row"]').map((row) => text(row)),
    projectSwitches: all('main [data-slot="project-switch"]').map((row) => `${text(row)} ${row.querySelector("[role=switch]")?.getAttribute("aria-checked")}`),
    banner: text(document.querySelector('main [data-slot="banner"]')),
    empty: text(document.querySelector('main [data-slot="plugins-empty"]')),
    emptyRow: text(document.querySelector('main [data-slot="empty-row"]')),
    removeRow: text(document.querySelector('main [data-slot="remove-row"]')),
    menu: menu ? { rows: all("button", menu).map(text), current: text(menu.querySelector("[aria-current]")) } : null,
    dialog: dialog
      ? {
          slot: dialog.getAttribute("data-dialog"),
          width: dialog.getBoundingClientRect().width,
          title: text(dialog.querySelector("h2")),
          description: text(dialog.querySelector("p")),
          text: text(dialog),
          agents: all('[data-slot="agent-row"]', dialog).map(text),
          buttons: all("button", dialog).map((button) => button.getAttribute("aria-label") ?? text(button)).filter(Boolean),
          radios: all("[role=radio]", dialog).map((radio) => `${text(radio)}${radio.getAttribute("aria-checked") === "true" ? " *" : ""}`),
        }
      : null,
    toasts: all('[data-slot="toast"]').map(text),
    alert: text(document.querySelector('main [role="alert"]')),
    overflow: main().scrollWidth > main().clientWidth,
  };
}

const project = (id: string | null) => usePlugins.getState().setProject(id);
async function reset(setKey: (key: number) => void, signIns = 1) {
  items = startingItems(signIns);
  empty = false;
  adding = null;
  document.querySelectorAll('[data-dialog] button[aria-label="Close"]').forEach((button) => (button as HTMLButtonElement).click());
  usePlugins.setState({ project: null });
  setKey(Date.now());
  usePlugins.getState().refresh();
  await pause();
}

/** Opens each drawn state from a fresh page. */
const OPENERS: Record<string, () => Promise<void>> = {
  L1: async () => {},
  L2: async () => click("Skills6"),
  L3: async () => project("p-brigadier"),
  L4: async () => project("p-steady"),
  L5: async () => click("Project"),
  L6: async () => click("Found in other apps3 in Claude Desktop and Cursor. Your agents can't use them until you bring them in."),
  L7: async () => {
    empty = true;
    usePlugins.getState().refresh();
  },
  D1: async () => click("Open linear"),
  D2: async () => {
    click("Details");
  },
  D3: async () => click("Open postgres"),
  G1: async () => {
    click("Add");
    await pause();
    type("What to add", "claude mcp add --transport http sentry https://mcp.sentry.dev/mcp");
  },
  G2: async () => {
    await OPENERS.G1!();
    await pause();
    click("Continue");
  },
  G3: async () => {
    project("p-brigadier");
    await pause();
    await OPENERS.G1!();
    await pause();
    click("Continue");
  },
  G4: async () => {
    await OPENERS.G2!();
    await pause();
    click("Add", document.querySelector("[data-dialog]")!);
  },
  G5: async () => click("Sign in"),
  G6: async () => click("Use everywhere"),
  G7: async () => {
    click("Open linear");
    await pause();
    click("Remove linear");
  },
};

async function drive(setKey: (key: number) => void) {
  const seen: Record<string, unknown> = {};
  const open = async (state: string, signIns = 1) => {
    await reset(setKey, signIns);
    await OPENERS[state]!();
    await pause(120);
    seen[state] = read();
  };
  for (const state of ["L1", "L2", "L3", "L4", "L5", "L6", "L7", "D1", "D2", "D3", "G1", "G2", "G3", "G4", "G6", "G7"]) await open(state);
  await open("G5", 4);

  // The project filter picks a project: the menu closes and the list follows it.
  await open("L5");
  all<HTMLButtonElement>('[data-slot="project-filter"] button').find((button) => text(button.querySelector(".truncate")) === "steady-app")!.click();
  await pause(120);
  seen.picked = read();

  // Switches: everywhere off, off in one project, and on in one project.
  await reset(setKey);
  const before = calls.length;
  flip("railway");
  await pause(120);
  seen.offEverywhere = read();
  project("p-brigadier");
  await pause(120);
  flip("pencil");
  await pause(120);
  flip("playwright");
  await pause(120);
  seen.inProject = read();
  seen.switchCalls = calls.slice(before);

  // A switch that can't keep to a project asks to use the item everywhere first, then flips.
  await reset(setKey);
  project("p-brigadier");
  await pause();
  click("Skills6");
  await pause();
  const asked = calls.length;
  flip("grill-me");
  await pause(120);
  seen.notSwitchable = read();
  click("Use everywhere", document.querySelector("[data-dialog]")!);
  await pause(300);
  seen.afterUseEverywhere = read();
  seen.useEverywhereCalls = calls.slice(asked);
  const undo = all<HTMLButtonElement>('[data-slot="toast"] button').find((button) => text(button) === "Undo");
  undo?.click();
  await pause();
  seen.undoCalls = calls.slice(asked);

  // Add: a paste that isn't anything says so; the picked project is the default; adding shows each agent.
  await reset(setKey);
  await OPENERS.G1!();
  type("What to add", "hello");
  await pause();
  click("Continue");
  await pause(120);
  seen.badPaste = read();
  await reset(setKey);
  await OPENERS.G4!();
  await pause(120);
  adding?.resolve(ok());
  await pause(120);
  seen.added = read();
  seen.addCalls = calls.filter((call) => call.startsWith("addPlugin"));

  // Details: the Projects card's switches, Remove, and a project note's fix.
  await open("D3");
  const detail = calls.length;
  click("brigadier-ai");
  await pause(120);
  click("Every project");
  await pause(120);
  seen.detailCalls = calls.slice(detail);
  await open("G7");
  click("Remove", document.querySelector("[data-dialog]")!);
  await pause(120);
  seen.removed = read();
  await reset(setKey);
  project("p-landing");
  await pause();
  click("Open pencil");
  await pause(120);
  seen.noted = read();
  click("Trust landing");
  await pause();
  seen.trustCalls = calls.filter((call) => call.startsWith("trust"));

  // Search keeps the rows that match.
  await reset(setKey);
  type("Search MCP servers", "rail");
  await pause(120);
  seen.searched = read();
  return seen;
}

function Fixture() {
  const [key, setKey] = useState(0);
  const [result, setResult] = useState("");
  useEffect(() => {
    if (params.has("drive")) {
      void drive(setKey).then((value) => setResult(JSON.stringify(value))).catch((cause: unknown) => setResult(JSON.stringify({ error: String(cause) })));
    } else {
      const state = params.get("state");
      if (state && OPENERS[state]) {
        const signIns = state === "G5" ? 4 : 1;
        void reset(setKey, signIns).then(() => pause()).then(() => OPENERS[state]!());
      }
    }
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-chrome text-foreground flex h-screen flex-col px-1 pb-1">
        <SidebarProvider keepOpen className="relative min-h-0 flex-1 flex-col">
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel><SettingsNav /></SidebarPanel>
            <div className="relative flex min-w-0 flex-1 flex-col">
              <SettingsTitlebarStrip><TerminalButton /></SettingsTitlebarStrip>
              <main className="bg-background border-divider rounded-e-page min-h-0 flex-1 overflow-hidden border-l">
                <PluginsPage key={key} />
              </main>
              <Toaster className="top-titlebar pt-2" />
            </div>
          </div>
          <TitlebarNav />
        </SidebarProvider>
      </div>
      {result && <pre id="plugins-result">{result}</pre>}
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
